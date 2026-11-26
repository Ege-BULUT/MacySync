//! End-to-end encryption. Small things on the bus (the offer, the picks, the transfer key) are sealed boxes
//! to a device's public key (X25519, XSalsa20-Poly1305). The files themselves go Mac to Mac as an
//! XChaCha20-Poly1305 STREAM under a random key only the two Macs know: every frame is authenticated, and a cut
//! stream fails instead of ending early.

use std::io::{self, Read, Write};

use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305};
use crypto_box::aead::{OsRng, rand_core::RngCore};
use crypto_box::{PublicKey, SecretKey};

pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

pub fn public(secret: &[u8; 32]) -> [u8; 32] {
    *SecretKey::from_bytes(*secret).public_key().as_bytes()
}

pub fn seal(to: &[u8; 32], msg: &[u8]) -> Vec<u8> {
    PublicKey::from_bytes(*to).seal(&mut OsRng, msg).expect("sealing never fails for in-memory input")
}

pub fn open(secret: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>, String> {
    SecretKey::from_bytes(*secret).unseal(sealed).map_err(|_| "this Mac's key can't open it".to_string())
}

const CHUNK: usize = 256 * 1024;
const LAST: u32 = 1 << 31;

/// Encrypts everything written to it; `finish` writes the closing frame. Starts with the stream's nonce.
pub struct Writer<W: Write> {
    enc: Option<EncryptorBE32<XChaCha20Poly1305>>,
    out: W,
    buf: Vec<u8>,
}

impl<W: Write> Writer<W> {
    pub fn new(key: &[u8; 32], mut out: W) -> io::Result<Self> {
        let nonce = random::<19>();
        out.write_all(&nonce)?;
        let enc = EncryptorBE32::from_aead(XChaCha20Poly1305::new(key.into()), nonce.as_ref().into());
        Ok(Writer { enc: Some(enc), out, buf: Vec::with_capacity(CHUNK) })
    }

    fn frame(&mut self, last: bool) -> io::Result<()> {
        let bad = |_| io::Error::other("encrypt");
        let ct = if last {
            self.enc.take().unwrap().encrypt_last(self.buf.as_slice()).map_err(bad)?
        } else {
            self.enc.as_mut().unwrap().encrypt_next(self.buf.as_slice()).map_err(bad)?
        };
        self.out.write_all(&(ct.len() as u32 | if last { LAST } else { 0 }).to_be_bytes())?;
        self.out.write_all(&ct)?;
        self.buf.clear();
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.frame(true)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

impl<W: Write> Write for Writer<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let n = data.len().min(CHUNK - self.buf.len());
        self.buf.extend_from_slice(&data[..n]);
        if self.buf.len() == CHUNK {
            self.frame(false)?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// Reads what a `Writer` wrote; an error, never a quiet end, when a frame was changed or the stream was cut.
pub struct Reader<R: Read> {
    dec: Option<DecryptorBE32<XChaCha20Poly1305>>,
    inp: R,
    buf: Vec<u8>,
    at: usize,
}

impl<R: Read> Reader<R> {
    pub fn new(key: &[u8; 32], mut inp: R) -> io::Result<Self> {
        let mut nonce = [0u8; 19];
        inp.read_exact(&mut nonce)?;
        let dec = DecryptorBE32::from_aead(XChaCha20Poly1305::new(key.into()), nonce.as_ref().into());
        Ok(Reader { dec: Some(dec), inp, buf: Vec::new(), at: 0 })
    }
}

impl<R: Read> Read for Reader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.at == self.buf.len() {
            let Some(dec) = self.dec.as_mut() else { return Ok(0) };
            let mut head = [0u8; 4];
            self.inp.read_exact(&mut head).map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "the transfer was cut"))?;
            let head = u32::from_be_bytes(head);
            if (head & !LAST) as usize > CHUNK + 64 {
                return Err(io::Error::other("bad frame"));
            }
            let mut ct = vec![0u8; (head & !LAST) as usize];
            self.inp.read_exact(&mut ct)?;
            let bad = |_| io::Error::other("a frame failed its check: wrong key or changed data");
            self.buf = if head & LAST != 0 { self.dec.take().unwrap().decrypt_last(ct.as_slice()).map_err(bad)? } else { dec.decrypt_next(ct.as_slice()).map_err(bad)? };
            self.at = 0;
        }
        let n = out.len().min(self.buf.len() - self.at);
        out[..n].copy_from_slice(&self.buf[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_box_opens_only_with_the_right_key() {
        let (a, b) = (random::<32>(), random::<32>());
        let s = seal(&public(&a), b"picks");
        assert_eq!(open(&a, &s).unwrap(), b"picks");
        assert!(open(&b, &s).is_err());
    }

    #[test]
    fn stream_round_trips_and_rejects_cuts_and_changes() {
        let key = random::<32>();
        let data: Vec<u8> = (0..CHUNK * 2 + 777).map(|i| (i * 31 % 251) as u8).collect();
        let mut w = Writer::new(&key, Vec::new()).unwrap();
        w.write_all(&data).unwrap();
        let wire = w.finish().unwrap();

        let mut back = Vec::new();
        Reader::new(&key, wire.as_slice()).unwrap().read_to_end(&mut back).unwrap();
        assert_eq!(back, data);

        // Cut after the first full frame: an error, not a short file.
        let cut = &wire[..19 + 4 + CHUNK + 16];
        assert!(Reader::new(&key, cut).unwrap().read_to_end(&mut Vec::new()).is_err());
        // One flipped byte.
        let mut bent = wire.clone();
        bent[40] ^= 1;
        assert!(Reader::new(&key, bent.as_slice()).unwrap().read_to_end(&mut Vec::new()).is_err());
        // Wrong key.
        assert!(Reader::new(&random::<32>(), wire.as_slice()).unwrap().read_to_end(&mut Vec::new()).is_err());
    }
}

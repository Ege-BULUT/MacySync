//! MacBook looks: the official models and colours (assets/catalog/macbooks.json), detection, and the drawing.
//! The drawings are original vector shapes from each model's traits (size, notch, Touch Bar, function row,
//! colours); no Apple artwork or logo.

use std::collections::HashMap;
use std::sync::OnceLock;

use rune::{Color, El, Shape, canvas, el};
use serde::Deserialize;

use crate::me::{Hardware, Look};

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Model {
    pub id: String,
    pub family: String,
    pub marketing_name: String,
    pub year: u32,
    pub chips: Vec<String>,
    pub screen_in: f32,
    pub size_label: String,
    pub model_identifiers: Vec<String>,
    pub design: Design,
    pub colors: Vec<Colour>,
    pub part_numbers: HashMap<String, String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Design {
    pub lid: String,
    pub notch: bool,
    pub touch_bar: bool,
    pub function_row: String,
    pub bezel: String,
    pub dimensions_mm: Dims,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Dims {
    pub width: f32,
    pub depth: f32,
    pub height: f32,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Colour {
    pub name: String,
    pub body_hex: String,
    pub body_hex_dark: String,
    pub keyboard_hex: String,
}

#[derive(Deserialize, Default)]
struct Catalog {
    models: Vec<Model>,
}

pub fn catalog() -> &'static [Model] {
    static C: OnceLock<Vec<Model>> = OnceLock::new();
    C.get_or_init(|| {
        let mut m = serde_json::from_str::<Catalog>(include_str!("../assets/catalog/macbooks.json")).map(|c| c.models).unwrap_or_default();
        m.retain(|m| !m.colors.is_empty());
        // Newest first, Pro before Air within a year, then by size.
        m.sort_by(|a, b| b.year.cmp(&a.year).then(b.family.cmp(&a.family)).then(a.screen_in.total_cmp(&b.screen_in)));
        m
    })
}

pub fn model(id: &str) -> Option<&'static Model> {
    catalog().iter().find(|m| m.id == id)
}

/// The model and colour this Mac reports; the colour from the part number when the catalog knows it.
pub fn guess(hw: &Hardware) -> Look {
    let mut hits: Vec<&Model> = catalog().iter().filter(|m| m.model_identifiers.iter().any(|i| i == &hw.model_id)).collect();
    // Some identifiers cover two chips (M3 Pro and M3 Max): prefer the one naming this chip.
    hits.sort_by_key(|m| !m.chips.iter().any(|c| hw.chip.contains(c.as_str())));
    let by_part = catalog().iter().find(|m| m.part_numbers.contains_key(&hw.part));
    let Some(m) = by_part.filter(|p| hits.iter().any(|h| h.id == p.id)).or(hits.first().copied()).or(catalog().first()) else {
        return Look::default();
    };
    let color = m.part_numbers.get(&hw.part).cloned().unwrap_or_else(|| m.colors[0].name.clone());
    Look { model: m.id.clone(), color }
}

pub fn resolve(look: &Look) -> Option<(&'static Model, &'static Colour)> {
    let m = model(&look.model).or(catalog().first())?;
    let c = m.colors.iter().find(|c| c.name == look.color).unwrap_or(&m.colors[0]);
    Some((m, c))
}

/// "MacBook Pro 13″ · M1 · 2020"
pub fn short_name(m: &Model) -> String {
    let fam = if m.family == "air" { "MacBook Air" } else { "MacBook Pro" };
    let chip = m.chips.first().map(String::as_str).unwrap_or("");
    format!("{fam} {}″ · {chip} · {}", m.size_label.trim_end_matches("-inch"), m.year)
}

pub fn hx(hex: &str, or: Color) -> Color {
    rune::color::hex(hex).unwrap_or(or)
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
}

pub const W: f32 = 1280.0;
pub const H: f32 = 780.0;

/// A layer over the whole window, in window coordinates.
pub fn full(shape: Shape) -> El {
    canvas(shape.view_box(W, H)).size(W, H).absolute(0.0, 0.0)
}

/// Where a drawn laptop's screen is, so the caller can put text on it.
pub struct Drawn {
    pub layers: Vec<El>,
    pub screen: [f32; 4],
}

/// An open MacBook seen from the front and a little above, its hinge centred on (`cx`, `hinge_y`). `width`
/// is a 16-inch lid's width; smaller models are drawn smaller by their real width.
pub fn laptop(key: &str, m: &Model, c: &Colour, cx: f32, hinge_y: f32, width: f32) -> Drawn {
    let black = Color { r: 0.004, g: 0.004, b: 0.005, a: 1.0 };
    let body = hx(&c.body_hex, Color { r: 0.5, g: 0.5, b: 0.52, a: 1.0 });
    let dark = hx(&c.body_hex_dark, mix(body, black, 0.45));
    let keys = hx(&c.keyboard_hex, black);
    let light = mix(body, Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, 0.35);
    let d = &m.design;
    let w = width * (if d.dimensions_mm.width > 0.0 { d.dimensions_mm.width / 355.7 } else { m.screen_in / 16.2 }).clamp(0.7, 1.0);
    let lid_h = w * 0.655;
    let (x0, y0) = (cx - w / 2.0, hinge_y - lid_h);
    let mut out = Vec::new();

    // Shadow under the deck.
    let depth = w * 0.25;
    out.push(el().key(format!("{key}-shadow")).absolute(cx - w * 0.6, hinge_y + depth * 0.55).size(w * 1.2, depth * 0.7).radius(depth).fill(Color { a: 0.55, ..black }).blur(18.0));

    // Lid: metal edge, black glass, the display.
    out.push(el().key(format!("{key}-lid")).absolute(x0, y0).size(w, lid_h).radius(w * 0.032)
        .linear_gradient(std::f32::consts::PI, [(0.0, light), (0.5, body), (1.0, dark)]));
    let rim = w * 0.009;
    out.push(el().key(format!("{key}-glass")).absolute(x0 + rim, y0 + rim).size(w - 2.0 * rim, lid_h - 2.0 * rim).radius(w * 0.026).fill(black));
    let bezel = if d.bezel == "thick" { w * 0.036 } else { w * 0.016 };
    let bottom_chin = if d.notch { bezel } else { w * 0.05 };
    let screen = [x0 + rim + bezel, y0 + rim + bezel, w - 2.0 * (rim + bezel), lid_h - 2.0 * rim - bezel - bottom_chin];
    out.push(el().key(format!("{key}-display")).absolute(screen[0], screen[1]).size(screen[2], screen[3]).radius(w * 0.012)
        .linear_gradient(2.4, [(0.0, Color { r: 0.012, g: 0.03, b: 0.075, a: 1.0 }), (0.55, Color { r: 0.004, g: 0.012, b: 0.03, a: 1.0 }), (1.0, Color { r: 0.02, g: 0.05, b: 0.11, a: 1.0 })]));
    if d.notch {
        let nw = w * 0.115;
        out.push(el().key(format!("{key}-notch")).absolute(cx - nw / 2.0, screen[1] - 1.0).size(nw, w * 0.027).radius(w * 0.008).fill(black));
    } else {
        out.push(el().key(format!("{key}-cam")).absolute(cx - 2.0, y0 + rim + bezel * 0.35).size(4.0, 4.0).radius(2.0).fill(Color { r: 0.03, g: 0.03, b: 0.05, a: 1.0 }));
    }

    // Deck in perspective: (u, t) with u across (-0.5..0.5) and t from the hinge to the front edge (0..1).
    let top = hinge_y + w * 0.008;
    let p = |u: f32, t: f32| [cx + u * w * (1.0 + 0.13 * t), top + t * depth];
    let quad = |u0: f32, u1: f32, t0: f32, t1: f32| vec![p(u0, t0), p(u1, t0), p(u1, t1), p(u0, t1)];
    let mut s = Shape::new();
    s = s.fill(vec![[x0 + w * 0.03, hinge_y - 1.0], [x0 + w * 0.97, hinge_y - 1.0], [x0 + w * 0.97, top + 1.0], [x0 + w * 0.03, top + 1.0]], mix(dark, black, 0.5));
    let bands = 8;
    for i in 0..bands {
        let (t0, t1) = (i as f32 / bands as f32, (i + 1) as f32 / bands as f32);
        s = s.fill(quad(-0.5, 0.5, t0, t1 + 0.002), mix(light, body, t0 * 0.9));
    }
    // Keyboard: a well a shade darker than the metal, black keys in rows.
    let (kt0, kt1, ku) = (0.07, 0.56, 0.41);
    s = s.fill(quad(-ku, ku, kt0, kt1), mix(body, dark, 0.55));
    let rows = 6;
    let fn_row = if d.touch_bar { 0.55 } else if d.function_row == "half-height" { 0.6 } else { 1.0 };
    let unit = (kt1 - kt0) / (rows as f32 - 1.0 + fn_row);
    let gap = 0.12;
    let mut t = kt0;
    for r in 0..rows {
        let h = if r == 0 { unit * fn_row } else { unit };
        let (a, b) = (t + unit * gap * 0.5, t + h - unit * gap * 0.5);
        if r == 0 && d.touch_bar {
            s = s.fill(quad(-ku + 0.012, ku - 0.012, a, b), Color { r: 0.01, g: 0.012, b: 0.02, a: 1.0 });
        } else if r == rows - 1 {
            // Bottom row: modifiers and the long space bar.
            let cuts = [-ku, -0.31, -0.23, -0.15, 0.13, 0.21, 0.29, ku];
            for k in cuts.windows(2) {
                s = s.fill(quad(k[0] + 0.004, k[1] - 0.004, a, b), keys);
            }
        } else {
            let n = 14;
            for k in 0..n {
                let (u0, u1) = (-ku + 2.0 * ku * k as f32 / n as f32, -ku + 2.0 * ku * (k + 1) as f32 / n as f32);
                s = s.fill(quad(u0 + 0.004, u1 - 0.004, a, b), keys);
            }
        }
        t += h;
    }
    // Trackpad, larger on the 14 and 16 inch.
    let tp = if m.screen_in > 14.0 { 0.2 } else { 0.165 };
    s = s.fill(quad(-tp, tp, 0.62, 0.95), mix(body, light, 0.25));
    s = s.stroke(quad(-tp, tp, 0.62, 0.95).into_iter().chain([p(-tp, 0.62)]).collect::<Vec<_>>(), 1.0, Color { a: 0.5, ..dark });
    // Front edge, thinner on the wedge, with the scoop for a thumb.
    let edge = if d.lid == "wedge" { w * 0.009 } else { w * 0.017 };
    let (fl, fr) = (p(-0.5, 1.0), p(0.5, 1.0));
    s = s.fill(vec![fl, fr, [fr[0] - 3.0, fr[1] + edge], [fl[0] + 3.0, fl[1] + edge]], dark);
    s = s.fill(vec![[cx - w * 0.07, fl[1]], [cx + w * 0.07, fl[1]], [cx + w * 0.05, fl[1] + edge * 0.6], [cx - w * 0.05, fl[1] + edge * 0.6]], mix(dark, black, 0.4));
    out.push(full(s).key(format!("{key}-deck")));
    Drawn { layers: out, screen }
}

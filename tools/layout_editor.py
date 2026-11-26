"""MacySync layout editor server: serves tools/layout-editor.html, reads and saves assets/layout.json, and renders
the real app with an unsaved layout for the preview.

usage: python3 -I tools/layout_editor.py [port]      then open http://127.0.0.1:8793/

Listens on 127.0.0.1 only. A save keeps the previous layout in tools/layout-backups/. A MacySync built from
this folder reads assets/layout.json while it runs, so an open window redraws with each save; "Open app"
starts one in demo mode (made-up Macs, nothing sent anywhere).
"""

import json
import os
import pathlib
import subprocess
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = pathlib.Path(__file__).resolve().parent.parent
LAYOUT = ROOT / "assets" / "layout.json"
BIN = ROOT / "target" / "debug" / "macysync"
SCENES = {"connect", "device", "models", "request", "sending", "receiving", "tr"}
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8793
app = None  # the demo window "Open app" started


def valid(layout: object) -> bool:
    if not isinstance(layout, dict):
        return False
    e = layout.get("elements")
    return (isinstance(layout.get("outline"), list) and len(layout["outline"]) >= 4 and isinstance(e, dict)
            and all(k in e for k in ("title", "laptop_left", "laptop_right", "left_panel", "right_panel", "controls", "dialog"))
            and isinstance(layout.get("conduit"), list) and len(layout["conduit"]) == 4 and isinstance(layout.get("flow"), dict))


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def send(self, code: int, body: bytes, kind: str) -> None:
        self.send_response(code)
        self.send_header("Content-Type", kind)
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = self.path.split("?")[0]
        if path in ("/", "/layout-editor.html"):
            page = (ROOT / "tools" / "layout-editor.html").read_text().replace("__LAYOUT__", LAYOUT.read_text())
            return self.send(200, page.encode(), "text/html; charset=utf-8")
        if path == "/layout":
            return self.send(200, LAYOUT.read_bytes(), "application/json")
        if path.startswith("/shots/"):
            name = pathlib.Path(path).name
            f = ROOT / "docs" / "shots" / name
            if f.suffix == ".png" and f.is_file():
                return self.send(200, f.read_bytes(), "image/png")
        self.send(404, b"not found", "text/plain")

    def do_POST(self):
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        except ValueError:
            return self.send(400, b'{"error":"bad json"}', "application/json")
        if self.path == "/save":
            layout = body.get("layout")
            if not valid(layout):
                return self.send(400, b'{"error":"that is not a whole layout"}', "application/json")
            backups = ROOT / "tools" / "layout-backups"
            backups.mkdir(exist_ok=True)
            (backups / f"layout-{time.strftime('%Y%m%d-%H%M%S')}.json").write_bytes(LAYOUT.read_bytes())
            LAYOUT.write_text(json.dumps(layout, indent=2) + "\n")
            return self.send(200, json.dumps({"ok": True, "path": str(LAYOUT)}).encode(), "application/json")
        if self.path == "/launch":
            global app
            if app is not None and app.poll() is None:
                return self.send(200, b'{"ok":true,"running":true}', "application/json")
            b = subprocess.run(["cargo", "build", "-j", "3"], cwd=ROOT, capture_output=True)
            if b.returncode != 0:
                return self.send(500, json.dumps({"error": b.stderr.decode(errors="replace")[-600:]}).encode(), "application/json")
            app = subprocess.Popen([str(BIN), "--demo"], cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            return self.send(200, b'{"ok":true}', "application/json")
        if self.path == "/render":
            layout, scene = body.get("layout"), body.get("scene", "connect")
            if not valid(layout) or scene not in SCENES:
                return self.send(400, b'{"error":"bad layout or scene"}', "application/json")
            if not BIN.exists():
                subprocess.run(["cargo", "build", "-j", "3"], cwd=ROOT, check=False)
            tmp = ROOT / "tools" / ".preview-layout.json"
            png = ROOT / "tools" / ".preview.png"
            tmp.write_text(json.dumps(layout))
            png.unlink(missing_ok=True)
            env = dict(os.environ, MACYSYNC_LAYOUT=str(tmp))
            r = subprocess.run([str(BIN), "--demo", "--snap", str(png), "--scene", scene, "--at", "1.5", "--scale", "1"],
                               env=env, capture_output=True, timeout=120)
            if not png.exists():
                msg = (r.stderr.decode(errors="replace") or "render failed")[-600:]
                return self.send(500, json.dumps({"error": msg}).encode(), "application/json")
            return self.send(200, png.read_bytes(), "image/png")
        self.send(404, b"not found", "text/plain")


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"MacySync layout editor: http://127.0.0.1:{PORT}/", flush=True)
    server.serve_forever()

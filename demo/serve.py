#!/usr/bin/env python3
"""Dev server for the zenpipe demo.

COOP/COEP headers are NOT enabled by default because they block
cross-origin resources (picsum images, etc.). Pass --coop to enable
them when testing SharedArrayBuffer / wasm-bindgen-rayon.
"""
import http.server, sys

enable_coop = '--coop' in sys.argv

class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        if enable_coop:
            self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
            self.send_header('Cross-Origin-Embedder-Policy', 'require-corp')
        super().end_headers()

port = int([a for a in sys.argv[1:] if a != '--coop'][0]) if len(sys.argv) > 1 and sys.argv[1] != '--coop' else 3847
coop_status = 'COOP/COEP enabled' if enable_coop else 'no COOP/COEP'
print(f'Serving at http://localhost:{port}/ ({coop_status})')
http.server.HTTPServer(('', port), Handler).serve_forever()

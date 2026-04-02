#!/usr/bin/env python3
"""Dev server with COOP/COEP headers for SharedArrayBuffer support."""
import http.server, sys

class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
        self.send_header('Cross-Origin-Embedder-Policy', 'require-corp')
        super().end_headers()

port = int(sys.argv[1]) if len(sys.argv) > 1 else 3847
print(f'Serving at http://localhost:{port}/ (COOP/COEP enabled)')
http.server.HTTPServer(('', port), Handler).serve_forever()

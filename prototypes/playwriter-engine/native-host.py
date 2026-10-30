#!/usr/bin/env python3
# Synthetic test native host. Framed stdin/stdout <-> private Unix socket.
import json, os, selectors, socket, struct, sys
sock = socket.socket(socket.AF_UNIX)
sock.connect(os.environ['CWW_PROTO_SOCKET'])
sel = selectors.DefaultSelector()
sel.register(sys.stdin.buffer, selectors.EVENT_READ, 'chrome')
sel.register(sock, selectors.EVENT_READ, 'parent')
chrome = bytearray()
parent = bytearray()
while True:
    for key, _ in sel.select():
        data = os.read(sys.stdin.fileno(), 65536) if key.data == 'chrome' else sock.recv(65536)
        if not data: sys.exit(0)
        if key.data == 'chrome':
            chrome.extend(data)
            while len(chrome) >= 4:
                size = struct.unpack('<I', chrome[:4])[0]
                if size > 1024 * 1024: sys.exit(1)
                if len(chrome) < 4 + size: break
                sock.sendall(chrome[4:4+size] + b'\n')
                del chrome[:4+size]
        else:
            parent.extend(data)
            while b'\n' in parent:
                body, _, parent = parent.partition(b'\n')
                sys.stdout.buffer.write(struct.pack('<I', len(body)) + body)
                sys.stdout.buffer.flush()

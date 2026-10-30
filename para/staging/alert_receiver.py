"""Private Alertmanager sink and an explicitly synthetic alert-delivery probe."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hmac
import json
import os
from pathlib import Path
import re
import sqlite3


class Receiver:
    def __init__(self, database, token):
        self.database = database
        self.token = token
        self.probe = 1
        with self.connect() as db:
            db.execute('CREATE TABLE IF NOT EXISTS receipt (id INTEGER PRIMARY KEY, '
                       'received TEXT DEFAULT CURRENT_TIMESTAMP, name TEXT, status TEXT, service TEXT)')

    def connect(self):
        return sqlite3.connect(self.database, timeout=10)


def handler(receiver):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass  # Never log authorization headers or upstream payloads.

        def reply(self, status, body, content_type='application/json'):
            data = body.encode() if isinstance(body, str) else json.dumps(body).encode()
            self.send_response(status)
            self.send_header('Content-Type', content_type)
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def authorized(self):
            return hmac.compare_digest(self.headers.get('Authorization', ''), 'Bearer ' + receiver.token)

        def do_GET(self):
            if self.path == '/healthz':
                return self.reply(200, {'status': 'ok'})
            if self.path == '/metrics':
                return self.reply(200, f'para_chat_staging_alert_probe {receiver.probe}\n', 'text/plain')
            if self.path != '/receipts':
                return self.reply(404, {})
            if not self.authorized():
                return self.reply(401, {})
            with receiver.connect() as db:
                rows = db.execute('SELECT id,received,name,status,service FROM receipt ORDER BY id DESC LIMIT 100').fetchall()
            return self.reply(200, {'receipts': [dict(zip(['id', 'received', 'name', 'status', 'service'], row)) for row in rows]})

        def do_POST(self):
            if self.path not in ('/alerts', '/probe'):
                return self.reply(404, {})
            if not self.authorized():
                return self.reply(401, {})
            try:
                length = int(self.headers.get('Content-Length', '0'))
                if not 0 < length <= 65536:
                    return self.reply(413, {})
                value = json.loads(self.rfile.read(length))
                if self.path == '/probe':
                    if value.get('up') not in (0, 1):
                        return self.reply(400, {})
                    receiver.probe = value['up']
                    return self.reply(200, {'up': receiver.probe})
                alerts = value.get('alerts')
                if not isinstance(alerts, list) or len(alerts) > 100:
                    return self.reply(400, {})
                rows = []
                for alert in alerts:
                    labels = alert.get('labels', {})
                    name, service, status = labels.get('alertname', ''), labels.get('service', ''), alert.get('status')
                    if not re.fullmatch(r'[A-Za-z][A-Za-z0-9_]{0,99}', name) or not re.fullmatch(r'[a-z0-9_-]{0,64}', service) or status not in ('firing', 'resolved'):
                        return self.reply(400, {})
                    rows.append((name, status, service))
                with receiver.connect() as db:
                    db.executemany('INSERT INTO receipt(name,status,service) VALUES (?,?,?)', rows)
                    db.execute('DELETE FROM receipt WHERE id NOT IN (SELECT id FROM receipt ORDER BY id DESC LIMIT 1000)')
                return self.reply(200, {'accepted': len(rows)})
            except (ValueError, TypeError, AttributeError):
                return self.reply(400, {})
    return Handler


if __name__ == '__main__':
    os.umask(0o077)
    token = Path('/run/secrets/alert_token').read_text().strip()
    if len(token) < 32:
        raise SystemExit('Missing private alert credential')
    ThreadingHTTPServer(('0.0.0.0', 39093), handler(Receiver('/data/receipts.sqlite', token))).serve_forever()

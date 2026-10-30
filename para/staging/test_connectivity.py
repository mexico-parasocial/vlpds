"""Security boundaries for private LAN configuration and alert delivery."""
from http.server import ThreadingHTTPServer
import json
from pathlib import Path
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from unittest.mock import patch

from alert_receiver import Receiver, handler
import lan_https
from operation_lock import operation


class ConnectivityTests(unittest.TestCase):
    def test_rejects_public_or_injected_hostname(self):
        for host in ['example.com', 'localhost', 'mac.local\nrespond 200', 'https://mac.local', 'mac.local/path']:
            with self.subTest(host=host), self.assertRaises(SystemExit):
                lan_https.prepare(host)

    def test_caddy_validates_private_config_without_installing_trust(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(lan_https, 'TLS', Path(directory)):
            lan_https.prepare('test-mac.local')
            config = (Path(directory) / 'Caddyfile').read_text()
            self.assertIn('skip_install_trust', config)
            self.assertIn('not remote_ip private_ranges', config)
            self.assertIn('respond @operator 404', config)
            self.assertIn('/_synapse/*', config)
            self.assertFalse((Path(directory) / 'caddy.pid').exists(), 'Preparation must not launch a network listener')

    def test_staging_operation_is_exclusive_and_released(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory)
            with operation(state):
                with self.assertRaises(SystemExit):
                    with operation(state): self.fail('An active lock was taken over')
            self.assertFalse((state / 'operation.lock').exists())

    def test_authenticated_receipts_are_bounded_and_do_not_store_message_text(self):
        with tempfile.TemporaryDirectory() as directory:
            receiver = Receiver(str(Path(directory) / 'receipts.sqlite'), 'fixture-secret')
            server = ThreadingHTTPServer(('127.0.0.1', 0), handler(receiver))
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            base = f'http://127.0.0.1:{server.server_port}'
            def request(path, body=None, token=None):
                headers = {'Content-Type': 'application/json'}
                if token: headers['Authorization'] = 'Bearer ' + token
                req = urllib.request.Request(base + path, data=json.dumps(body).encode() if body is not None else None, headers=headers)
                return json.load(urllib.request.urlopen(req, timeout=2))
            try:
                for path, body in [('/receipts', None), ('/alerts', {'alerts': []}), ('/probe', {'up': 0})]:
                    with self.subTest(path=path), self.assertRaises(urllib.error.HTTPError) as error:
                        request(path, body)
                    self.assertEqual(error.exception.code, 401)
                request('/alerts', {'alerts': [{'status': 'firing', 'labels': {'alertname': 'FixtureAlert', 'service': 'bridge', 'private': 'excluded'}, 'annotations': {'message': 'DO NOT STORE'}}]}, 'fixture-secret')
                receipts = request('/receipts', token='fixture-secret')['receipts']
                self.assertEqual(receipts[0]['name'], 'FixtureAlert')
                self.assertNotIn('DO NOT STORE', json.dumps(receipts))
                self.assertNotIn('excluded', json.dumps(receipts))
                with self.assertRaises(urllib.error.HTTPError) as error:
                    request('/alerts', {'alerts': [{'status': 'firing', 'labels': {'alertname': 'FixtureAlert', 'service': 'contains private data'}}]}, 'fixture-secret')
                self.assertEqual(error.exception.code, 400)
            finally:
                server.shutdown()
                server.server_close()
                thread.join(timeout=3)


if __name__ == '__main__': unittest.main()

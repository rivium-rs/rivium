These are synthetic self-signed TLS credentials for loopback tests only. They are
public test material, not production credentials, and are excluded from packages.
Generated on 2026-10-16 for localhost, RSA 2048, validity 3650 days, CA=false.

Regeneration:

```sh
openssl req -x509 -newkey rsa:2048 -nodes -keyout key.pem -out cert.pem \
  -days 3650 -subj '/CN=localhost' -addext 'subjectAltName=DNS:localhost' \
  -addext 'basicConstraints=critical,CA:FALSE'
openssl x509 -in cert.pem -outform DER -out localhost.der
openssl pkcs8 -topk8 -nocrypt -in key.pem -outform DER -out localhost-key.der
```

# TLS smoke test data

A throwaway certificate chain for the in-memory TLS handshake in `src/lib.rs`. **Not a secret:**
the key protects nothing; it only lets the test exercise certificate verification with ring.

| File | Content |
| --- | --- |
| `ca.der` | Self-signed test CA (`CN=smoke test CA`), ECDSA P-256 |
| `leaf.der` | `CN=localhost` with `subjectAltName=DNS:localhost`, signed by the CA, valid until 2126 |
| `leaf.key.der` | The leaf's PKCS#8 private key |

Regenerate with OpenSSL:

```bash
openssl ecparam -name prime256v1 -genkey -noout -out ca.key
openssl req -x509 -new -key ca.key -sha256 -days 36500 -subj "/CN=smoke test CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" -out ca.pem
openssl ecparam -name prime256v1 -genkey -noout -out leaf.key
openssl req -new -key leaf.key -subj "/CN=localhost" -out leaf.csr
printf 'subjectAltName=DNS:localhost\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n' > ext.cnf
openssl x509 -req -in leaf.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 36500 -sha256 -extfile ext.cnf -out leaf.pem
openssl x509 -in ca.pem -outform der -out ca.der
openssl x509 -in leaf.pem -outform der -out leaf.der
openssl pkcs8 -topk8 -nocrypt -in leaf.key -outform der -out leaf.key.der
```

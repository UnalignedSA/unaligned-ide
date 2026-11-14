# Test keys

Every file here is a throwaway key made for the tests of this crate and for
nothing else. None of them protects anything: they were never installed on a
machine or a service, and their passphrases are written in the tests.

Do not use them, and do not report them as leaked secrets.

- `putty_*.ppk` were written by PuTTYgen 0.78 (`puttygen -t … --ppk-param version=…`).
  `putty_v2_ed25519_latin1.ppk` is not valid UTF-8 on purpose: its comment and
  its passphrase are in Latin-1, as an old PuTTY for Windows wrote them.
- `openssh_*.key`, `openssh_*.pub` and `pem_*.pem` were written by `ssh-keygen`
  (OpenSSH 10.5 with LibreSSL, and OpenSSH 9.2 with OpenSSL 3 for the DSA key
  and the keys that name their curve).

The values the tests expect (public keys, and a hash of the private part of
each converted PuTTY key) were recorded from those same tools: the public key
PuTTYgen prints, and PuTTYgen's own OpenSSH export as rewritten by
`ssh-keygen -p`.

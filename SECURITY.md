# Security

## Reporting a problem

Please do not open a public issue for a security problem. Write through the contact page of Unaligned, <https://unaligned.pw/contact>, and say that it is about the security of Unaligned IDE. Describe what you found and how to see it.

## What the project signs and checks

- **Updates.** An installed copy installs a new version only when the release's list of files is signed with the project's update key and the download has the exact size and SHA-256 that list gives: [`docs/unaligned/UPDATES.md`](docs/unaligned/UPDATES.md).
- **The remote server.** The program the application puts on an SSH host comes from the same signed list and is checked on your machine before it is uploaded: [`docs/unaligned/REMOTE.md`](docs/unaligned/REMOTE.md).
- **Not signed for the operating system.** The Windows installer has no certificate and the macOS application is not notarised yet, so a file downloaded by hand is checked by nothing but the address you got it from. Download from this repository's Releases page only.

## What the application sends

No account, no telemetry, no crash reports. What it contacts is listed in the README, section Privacy.

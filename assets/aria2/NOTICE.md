# Embedded aria2 downloader

MiPCManager_Patch embeds the unmodified x64 `aria2c.exe` from the official
[aria2 1.37.0 release](https://github.com/aria2/aria2/releases/tag/release-1.37.0).
It is gzip-compressed to reduce the size of each executable. The installer itself
is downloaded separately from Xiaomi; it is not embedded or redistributed here.

| Item | Value |
| --- | --- |
| Upstream commit | `02f2d0d8472b3c38c29b4dba8c75ebd5fdd2899a` |
| Official artifact | `aria2-1.37.0-win-64bit-build1.zip` |
| Artifact SHA-256 | `67d015301eef0b612191212d564c5bb0a14b5b9c4796b76454276a4d28d9b288` |
| `aria2c.exe` size | 5,649,408 bytes |
| `aria2c.exe` SHA-256 | `be2099c214f63a3cb4954b09a0becd6e2e34660b886d4c898d260febfe9d70c2` |
| Embedded `aria2c.exe.gz` size | 2,440,413 bytes |
| Embedded gzip SHA-256 | `d96200be629c5fb72f6bc977caeced59db352a1c1e4e5040582f33854affcc3e` |

Artifact URL:
https://github.com/aria2/aria2/releases/download/release-1.37.0/aria2-1.37.0-win-64bit-build1.zip

To reproduce the embedded gzip, extract `aria2c.exe` from that artifact and use
Python 3.12 on Linux: `gzip.compress(exe_bytes, compresslevel=9, mtime=0)`. The downloader verifies
the decompressed executable before starting it. No user-installed aria2 or
user-provided aria2 configuration is used.

## License and corresponding source

Copyright (C) 2006, 2019 Tatsuhiro Tsujikawa and the contributors listed in
[AUTHORS](AUTHORS). aria2 is licensed under GPL-2.0-or-later with the upstream
OpenSSL linking exception. This redistribution uses GPLv3, matching
MiPCManager_Patch. [COPYING](COPYING), [LICENSE.OpenSSL](LICENSE.OpenSSL), and
[README.mingw](README.mingw) are copied unchanged from the official Windows ZIP.

Every Release includes `aria2-1.37.0-sources.zip` alongside the GUI and CLI
executables. This separate archive contains these notices, the GPLv3 license,
aria2's source, its Windows build recipe, and the source archives of all six
statically linked libraries: GMP 6.3.0, Expat 2.5.0, SQLite 3.43.1, zlib 1.3,
c-ares 1.19.1, and libssh2 1.11.0. Their licenses and copyright notices are
included in their respective source archives. The source archive is not embedded
in the application and is not needed to run it.

The exact source download URLs and SHA-256 hashes are maintained in
[sources.json](sources.json). All are freely accessible from the upstream
servers; the release packaging verifies each hash before publishing the archive.
The build recipe is pinned to the same aria2 commit. To rebuild the x64 binary:

```text
docker build -f Dockerfile.mingw --build-arg HOST=x86_64-w64-mingw32 --build-arg ARIA2_VERSION=02f2d0d8472b3c38c29b4dba8c75ebd5fdd2899a --build-arg ARIA2_REF=commits/02f2d0d8472b3c38c29b4dba8c75ebd5fdd2899a -t aria2-mingw .
docker run --rm -v /path/to/output:/out aria2-mingw cp /aria2/src/aria2c.exe /out
```

For historical versions whose download URLs moved, use the included source
archives with the build recipe. Rebuilding with a different toolchain is not
claimed to reproduce the official binary byte for byte. Redistribution must
continue to provide the source archive and these notices with the executables.

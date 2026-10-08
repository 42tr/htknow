# RAR regression fixtures

`rar3-*.rar`, `rar5-*.rar` come from [markokr/rarfile test/files](https://github.com/markokr/rarfile/tree/master/test/files), under the ISC license reproduced in `LICENSE.rarfile`. The `rar3` samples use the pre-RAR5 format (also used by RAR4). Encrypted RAR5 samples use the password `password`.

- `rar3-subdirs.rar`, `rar5-subdirs.rar`: nested directories, empty directories, spaces and Unicode names.
- `rar5-crc.rar`: two 2048-byte files with identical content, one compressed and one stored.
- `rar5-solid.rar`: two identical 2048-byte files in a solid archive.
- `rar5-psw.rar`: encrypted file data.
- `rar5-hpsw.rar`: encrypted headers and file data.
- `rar5-evil-symlink-traversal.rar`: a symlink to the parent directory followed by a file beneath that symlink.

`rar4-crypted.rar` and `rar4-comment-hpw-password.rar` come from [muja/unrar.rs data](https://github.com/muja/unrar.rs/tree/master/data), under the MIT license reproduced in `LICENSE.unrar-rs`. They contain the 17-byte `.gitignore` file, use passwords `unrar` and `password` respectively, and cover data encryption and header encryption in the pre-RAR5 format.

`paths.rar` and `volume.part1.rar` are project-generated stored RAR4-format fixtures. Each header starts with the low 16 bits of the CRC32 of the remaining header bytes; file checksums are CRC32 of the stored data. `paths.rar` contains five files with the content `hello rar\n`: a Chinese name, a Windows separator path, and three unsafe paths (`../outside.txt`, `/absolute.txt`, `C:\outside.txt`). Unicode names use the RAR4 encoded-name field with literal UTF-16 code units (mode 2). `volume.part1.rar` has the main-header volume and first-volume flags (`0x101`) and no file entries.

Tests use these checked-in files directly; no RAR CLI or network access is required.

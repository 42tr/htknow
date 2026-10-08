These regression archives come from sevenz-rust2 0.23.0, tests/resources,
licensed under Apache-2.0 (see LICENSE-Apache-2.0).
Source: https://github.com/hasenbanck/sevenz-rust2/tree/main/tests/resources

- single_file_with_content_lzma.7z: LZMA file with an unencoded header.
- solid.7z: a solid archive produced independently of the test writer.
- encrypted.7z: encrypted archive, password `sevenz-rust`.
- bzip2_file.7z, ppmd.7z: alternative compression algorithms.

Other archive cases (Unicode paths/passwords, encrypted headers, unsafe paths,
limits and corrupted data) are generated in src/archive/sevenz.rs tests.

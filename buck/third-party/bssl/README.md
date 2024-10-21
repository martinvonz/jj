## BoringSSL Notes

When you update this package, including the git commit and archive hash, make
sure you also re-download the proper `BUILD.generated.bzl` file, which is
produced automatically by a bot in the upstream `master-with-bazel` branch:

- https://github.com/google/boringssl/tree/master-with-bazel

```
curl -Lo \
    buck/third-party/cxx/bssl/BUILD.generated.bzl \
    https://raw.githubusercontent.com/google/boringssl/.../BUILD.generated.bzl
```

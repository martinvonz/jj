
def cxx_library(**kwargs):
    fail('use load("@root//buck/shims/jj.bzl", "jj") and call jj.cxx_library() instead')

def cxx_binary(**kwargs):
    fail('use load("@root//buck/shims/jj.bzl", "jj") and call jj.cxx_binary() instead')

def prebuilt_cxx_library(**kwargs):
    fail('use load("@root//buck/shims/jj.bzl", "jj") and call jj.prebuilt_cxx_library() instead')

def rust_library(**kwargs):
    fail('use load("@root//buck/shims/jj.bzl", "jj") and call jj.rust_library() instead')

def rust_binary(**kwargs):
    fail('use load("@root//buck/shims/jj.bzl", "jj") and call jj.rust_binary() instead')

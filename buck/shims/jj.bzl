
# version of JJ for the user to see
# XXX FIXME (aseipp): unify this with Cargo.toml, somehow?
JJ_VERSION = '0.18.0'

# wrap native.rust_*, but provide some extra default args
def _jj_rust_rule(rule_name: str, **kwargs):
    edition = kwargs.pop('edition', '2021')
    env = {
        'JJ_VERSION': JJ_VERSION,
    } | kwargs.pop('env', {})
    rustc_flags = [
        '--cfg=buck_build',
    ] + kwargs.pop('rustc_flags', [])

    fn = getattr(native, rule_name)
    fn(
        edition = edition,
        env = env,
        rustc_flags = rustc_flags,
        # XXX FIXME (aseipp): incrementality should be optional based on the
        # build mode, but also not dependent directly on read_config, so as not
        # to invalidate cache hits. fix when real modes are possible
        incremental_enabled = True,
        **kwargs,
    )

def _jj_rust_library(**kwargs):
    _jj_rust_rule('rust_library', **kwargs)

def _jj_rust_binary(**kwargs):
    _jj_rust_rule('rust_binary', **kwargs)

def _jj_cxx_library(**_kwargs):
    native.cxx_library(**_kwargs)

def _jj_cxx_binary(**_kwargs):
    native.cxx_binary(**_kwargs)

def _jj_prebuilt_cxx_library(**_kwargs):
    native.prebuilt_cxx_library(**_kwargs)

jj = struct(
    rust_library = _jj_rust_library,
    rust_binary = _jj_rust_binary,

    cxx_library = _jj_cxx_library,
    prebuilt_cxx_library = _jj_prebuilt_cxx_library,
    cxx_binary = _jj_cxx_binary,
)

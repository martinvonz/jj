def constraint_with_values(name, values, **kwargs):
    """Declare a constraint setting with a set of values."""

    native.constraint_setting(
        name = name,
        **kwargs,
    )

    for value in values:
        native.constraint_value(
            name = value,
            constraint = ":{}".format(name),
            **kwargs,
        )

def _execution_platform_impl(ctx: AnalysisContext) -> list[Provider]:
    constraints = dict()
    constraints.update(ctx.attrs.cpu_configuration[ConfigurationInfo].constraints)
    constraints.update(ctx.attrs.os_configuration[ConfigurationInfo].constraints)
    for x in ctx.attrs.constraints:
        constraints.update(x[ConfigurationInfo].constraints)

    cfg = ConfigurationInfo(constraints = constraints, values = {})

    name = ctx.label.raw_target()
    re_enabled = ctx.attrs.remote_enabled
    os = ctx.attrs.os

    re_properties = {}
    if re_enabled:
        if os == "linux":
            re_properties = {
                "OSFamily": "Linux",
                "container-image": "nix-bb-runner",
            }
        elif os == "macos":
            re_enabled = False # TODO FIXME (aseipp): support macos
            re_properties = {
                "OSFamily": "Darwin",
                # ...
            }
        elif os == "windows":
            re_enabled = False # TODO FIXME (aseipp): support windows
            re_properties = {
                "OSFamily": "Windows",
                # ...
            }
        else:
            fail("Invalid OS for remote execution: {}".format(os))

    # Configuration of how a command should be executed.
    force_remote_exe = re_enabled and (read_root_config("buck2_re_client", "force_remote", "false") == "true")
    allow_local_fallback = re_enabled and (read_root_config("buck2_re_client", "allow_local_fallback", "false") == "true")
    exe_cfg = CommandExecutorConfig(
        # Whether to use local execution for this execution platform. If both
        # remote_enabled and local_enabled are True, we will use the hybrid
        # executor
        local_enabled = True,

        # Whether to use remote execution for this execution platform
        remote_enabled = re_enabled,

        # Whether to use the "limited" hybrid executor. If the hybrid
        # executor is active, by default, it will race the two executors
        # to completion until one finishes. If the limited hybrid executor
        # is enabled, then both are exposed, but only the preferred one
        # is chosen. Finally, if allow_limited_hybrid_fallbacks is true,
        # then if the preferred executor fails, the other executor will be
        # tried.
        use_limited_hybrid = force_remote_exe,
        allow_limited_hybrid_fallbacks = allow_local_fallback,
        experimental_low_pass_filter = re_enabled and not force_remote_exe,

        # Use and query the RE cache
        remote_cache_enabled = re_enabled,

        # Whether to upload local actions to the RE cache
        allow_cache_uploads = re_enabled,

        # Whether to use Windows path separators in command line arguments
        use_windows_path_separators = os == "windows",

        # Properties for remote execution for this platform. BuildBarn will
        # match these properties against the properties of the remote workers it
        # has attached; all fields must match.
        remote_execution_properties = re_properties,

        # The use case to use when communicating with RE.
        remote_execution_use_case = "buck2-default",

        # How to express output paths to RE. This is used internally for the
        # FB RE implementation and the FOSS implementation; strict means that
        # the RE implementation should expect the output paths to be specified
        # as files or directories in all cases, and that's what the Remote
        # Execution API expects. So this will never change.
        remote_output_paths = "strict",

        # Max file size that the RE system can support
        remote_execution_max_input_files_mebibytes = None, # default: 30 * 1024 * 1024 * 1024

        # Max time we're willing to wait in the RE queue
        remote_execution_queue_time_threshold_s = None,

        remote_dep_file_cache_enabled = False,
    )

    exe_platform = ExecutionPlatformInfo(
        label = name,
        configuration = cfg,
        executor_config = exe_cfg,
    )

    return [
        DefaultInfo(),
        exe_platform,
        PlatformInfo(label = str(name), configuration = cfg),
        ExecutionPlatformRegistrationInfo(platforms = [exe_platform]),
    ]

__execution_platform = rule(
    impl = _execution_platform_impl,
    attrs = {
        "cpu_configuration": attrs.dep(providers = [ConfigurationInfo]),
        "os_configuration": attrs.dep(providers = [ConfigurationInfo]),
        "constraints": attrs.list(attrs.dep(providers = [ConfigurationInfo]), default = []),
        "remote_enabled": attrs.bool(default = False),
        "cpu": attrs.string(),
        "os": attrs.string(),
    },
)

def _host_cpu_configuration() -> str:
    arch = host_info().arch
    if arch.is_aarch64:
        return "config//cpu:arm64"
    else:
        return "config//cpu:x86_64"

def _host_os_configuration() -> str:
    os = host_info().os
    if os.is_macos:
        return "config//os:macos"
    elif os.is_windows:
        return "config//os:windows"
    else:
        return "config//os:linux"

def generate_platforms(variants, constraints=[]):
    """Generate execution platforms for the given variants, as well as a default
    execution platform matching the host platform."""

    # We want to generate a remote-execution capable variant of every supported
    # platform (-re suffix) as well as a local variant (-local suffix) for the
    # current execution platform that buck2 is running on.
    default_alias_prefix = "none//fake:nonexistent"
    for (cpu, os) in variants:
        cpu_configuration = "config//cpu:{}".format(cpu)
        os_configuration = "config//os:{}".format(os)

        # always generate generate a remote-execution variant
        __execution_platform(
            name = "{}-{}-remote".format(cpu, os),
            cpu_configuration = cpu_configuration,
            os_configuration = os_configuration,
            constraints = constraints,
            remote_enabled = True,
            cpu = cpu,
            os = os,
        )

        # and, if it matches the host platform: generate a -local variant, too,
        # so builds can happen locally as well.
        if _host_cpu_configuration() == cpu_configuration and _host_os_configuration() == os_configuration:
            default_alias_prefix = "root//buck/platforms:{}-{}".format(cpu, os)
            __execution_platform(
                name = "{}-{}-local".format(cpu, os),
                cpu_configuration = cpu_configuration,
                os_configuration = os_configuration,
                constraints = constraints,
                remote_enabled = False,
                cpu = cpu,
                os = os,
            )

    # default to remote compilation being turned off; enable it if the
    # buck2_re_client.default_enabled option is set to "true", but only on
    # supported platforms. set it to "force-true" to unconditionally enable it,
    # which is useful for testing and platform bringup.
    remote_default = False
    re_default_enabled = read_root_config("buck2_re_client", "default_enabled", "false")

    if re_default_enabled == "true":
        if host_info().os.is_linux and not host_info().arch.is_aarch64:
            remote_default = True
        else:
            # TODO FIXME (aseipp): enable on all platforms
            remote_default = False
    elif re_default_enabled == "false":
        remote_default = False
    elif re_default_enabled == "force-true":
        remote_default = True
    else:
        fail('Invalid buck2_re_client.default_enabled setting: {}'.format(re_default_enabled))

    # now, alias() it to the proper local or remote build
    native.alias(
        name = "default",
        actual = "{}-{}".format(default_alias_prefix, "remote" if remote_default else "local"),
    )

# NOTE: keep the list of default platforms here instead of in BUILD. why?
# because it keeps all the internal specifics like _host_cpu_configuration and
# _host_os_configuration literals all in one spot.
default_platforms = [
    ("arm64", "linux"),
    ("arm64", "macos"),
   #("arm64", "windows"),
    ("x86_64", "linux"),
   #("x86_64", "macos"),
    ("x86_64", "windows"),
]

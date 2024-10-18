
load(
    "@prelude//python_bootstrap:python_bootstrap.bzl",
    "PythonBootstrapToolchainInfo",
)

def _simple_exe_download(ctx):
    output = ctx.actions.declare_output(ctx.label.name)
    ctx.actions.download_file(
        output.as_output(),
        ctx.attrs.url,
        sha256 = ctx.attrs.sha256,
        is_executable = True,
    )
    return [
        DefaultInfo(default_output = output),
        RunInfo(args = cmd_args([output])),
    ]

simple_exe_download = rule(
    impl = _simple_exe_download,
    attrs = {
        "url": attrs.string(),
        "sha256": attrs.string(),
    },
)

def _standalone_python_download(ctx):
    output = ctx.actions.declare_output("tar", ctx.label.name)
    ctx.actions.download_file(
        output.as_output(),
        ctx.attrs.url,
        sha256 = ctx.attrs.sha256,
    )

    out_dir = ctx.actions.declare_output("dir", ctx.label.name, dir = True)
    ctx.actions.run(
        cmd_args([
            ctx.attrs.smoltar[DefaultInfo].default_outputs[0],
            "-x",
            output,
            out_dir.as_output(),
        ]),
        category = "system_tar"
    )

    bin = out_dir.project(ctx.attrs.exe)
    return [
        DefaultInfo(
            default_output = out_dir,
            sub_targets = {
                ctx.attrs.exe: [ DefaultInfo(default_output = bin) ],
            }
        ),
        RunInfo(args = cmd_args([bin])),
    ]

standalone_python_download = rule(
    impl = _standalone_python_download,
    attrs = {
        "url": attrs.string(),
        "sha256": attrs.string(),
        "exe": attrs.string(),
        "smoltar": attrs.exec_dep(),
    },
)

def _standalone_python_bootstrap(ctx):
    args = cmd_args([ctx.attrs.interpreter])
    return [
        DefaultInfo(),
        RunInfo(args = args),
        PythonBootstrapToolchainInfo(interpreter = args),
    ]

standalone_python_bootstrap_toolchain = rule(
    impl = _standalone_python_bootstrap,
    attrs = {
        "interpreter": attrs.arg(),
    },
    is_toolchain_rule = True,
)

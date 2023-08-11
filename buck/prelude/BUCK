# Done to avoid triggering a lint rule that replaces glob with an fbcode macro
load(":defs.bzl", "export_prelude")

globby = glob

srcs = globby(
    ["**"],
    # Context: https://fb.workplace.com/groups/buck2users/posts/3121903854732641/
    exclude = ["**/.pyre_configuration.local"],
)

export_prelude(srcs = srcs)

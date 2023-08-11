# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import os


def normalize_and_write_deps(deps, dst_path):
    cwd = os.getcwd() + os.sep
    cwd = cwd.replace("\\", "/")
    normalized_deps = []
    for dep in deps:
        # The paths we get sometimes include "../" components, so get rid
        # of those because we want ForwardRelativePath here.
        dep = os.path.normpath(dep).replace("\\", "/")

        if os.path.isabs(dep):
            if dep.startswith(cwd):
                # The dep file included a path inside the build root, but
                # expressed an absolute path. In this case, rewrite it to
                # be a relative path.
                dep = dep[len(cwd) :]
            else:
                # The dep file included a path to something outside the
                # build root. That's bad (actions shouldn't depend on
                # anything outside the build root), but that dependency is
                # therefore not tracked by Buck2 (which can only see things
                # in the build root), so it cannot be represented as a
                # dependency and therefore we don't include it (event if we
                # could include it, this could never cause a miss).
                continue

        normalized_deps.append(dep)

    with open(dst_path, "w") as f:
        for dep in normalized_deps:
            f.write(dep)
            f.write("\n")

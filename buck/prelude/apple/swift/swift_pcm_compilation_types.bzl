# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

SwiftPCMUncompiledInfo = provider(fields = [
    "name",
    "is_transient",  # If True represents a transient apple_library target, that can't be compiled into pcm, but which we need to pass up for BUCK1 compatibility, because it can re-export some deps.
    "exported_preprocessor",  # CPreprocessor
    "exported_deps",  # [Dependency]
    "propagated_preprocessor_args_cmd",  # cmd_args
    "uncompiled_sdk_modules",  # [str] a list of required sdk modules
])

# A tset can't be returned from the rule, so we need to wrap it into a provider.
WrappedSwiftPCMCompiledInfo = provider(fields = [
    "tset",  # Tset of `SwiftPCMCompiledInfo`
])

SwiftPCMCompiledInfo = provider(fields = [
    "name",
    "pcm_output",  # artefact
    "exported_preprocessor",  # CPreprocessor which we need to keep around to be able to access modulemap path.
])

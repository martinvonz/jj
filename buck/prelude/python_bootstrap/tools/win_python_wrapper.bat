@REM Copyright (c) Meta Platforms, Inc. and affiliates.
@REM
@REM This source code is licensed under both the MIT license found in the
@REM LICENSE-MIT file in the root directory of this source tree and the Apache
@REM License, Version 2.0 found in the LICENSE-APACHE file in the root directory
@REM of this source tree.

:: A wrapper to set PYTHONPATH and run a python command with the specified interpreter
:: First arg: paths to library that should be made available
:: Second arg: path to python interpreter
:: Third arg: path to python file that should be run
:: Fourth and onwards: any other arg that should be passed
@echo off

:: See https://stackoverflow.com/questions/382587/how-to-get-batch-file-parameters-from-nth-position-on
setlocal enabledelayedexpansion
set args=;;;;;;%*
set args=!args:;;;;;;%1 =!

set PYTHONPATH=%1
%args%

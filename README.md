# New Project Template

This repository contains a template that can be used to seed a repository for a
new Google open source project.

See [go/releasing](http://go/releasing) (available externally at
https://opensource.google/docs/releasing/) for more information about
releasing a new Google open source project.

This template uses the Apache license, as is Google's default.  See the
documentation for instructions on using alternate license.

## How to use this template

1. Clone it from GitHub.
    * There is no reason to fork it.
1. Create a new local repository and copy the files from this repo into it.
1. Modify README.md and docs/contributing.md to represent your project, not the
   template project.
1. Develop your new project!

``` shell
git clone https://github.com/google/new-project
mkdir my-new-thing
cd my-new-thing
git init
cp -r ../new-project/* ../new-project/.github .
git add *
git commit -a -m 'Boilerplate for new Google open source project'
```

## Source Code Headers

Every file containing source code must include copyright and license
information. This includes any JS/CSS files that you might be serving out to
browsers. (This is to help well-intentioned people avoid accidental copying that
doesn't comply with the license.)

Apache header:

    Copyright 2020 Google LLC

    Licensed under the Apache License, Version 2.0 (the "License");
    you may not use this file except in compliance with the License.
    You may obtain a copy of the License at

        https://www.apache.org/licenses/LICENSE-2.0

    Unless required by applicable law or agreed to in writing, software
    distributed under the License is distributed on an "AS IS" BASIS,
    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
    See the License for the specific language governing permissions and
    limitations under the License.

# Distributed ThinLTO in Buck2
Sean Gillespie, April 2022

This document is a technical overview into Buck2's implementation of a distributed ThinLTO.
Like all rules in Buck2, this implementation is written entirely in Starlark, contained in
`dist_lto.bzl` (in this same directory).

## Motivation

First, I highly recommend watching [Teresa Johnson's CppCon2017 talk about ThinLTO](https://www.youtube.com/watch?v=p9nH2vZ2mNo),
which covers the topics in this section in much greater detail than I can.

C and C++ have long enjoyed significant optimizations at the hands of compilers. However, they have also
long suffered a fundamental limitation; a C or C++ compiler can only optimize code that it sees in a single
translation unit. For a language like C or C++, this means in practice that only code that is included via
the preprocessor or specified in the translation unit can be optimized as a single unit. C and C++ compilers
are unable to inline functions that are defined in different translation units. However, a crucial advantage
of this compilation model is that all C and C++ compiler invocations are *completely parallelizable*; despite
sacrificing some code quality, C and C++ compilation turns into a massively parallel problem with a serial
link step at the very end.

```
flowchart LR;
    header.h;

    header.h --> a.cpp;
    header.h -->|#include| b.cpp;
    header.h --> c.cpp;

    a.cpp --> a.o;
    b.cpp -->|clang++ -O2| b.o;
    c.cpp --> c.o;

    a.o --> main;
    b.o -->|ld| main;
    c.o --> main;
```

([Rendered](https://fburl.com/mermaid/rzup8o32). Compilation and optimization of a, b, and c can proceed in parallel.)


In cases where absolute performance is required, though, the inability to perform cross-translation-unit
(or "cross-module", in LLVM parlance) optimizations becomes more of a problem. To solve this, a new compilation
paradigm was designed, dubbed "Link-Time Optimization" (LTO). In this scheme, a compiler will not produce machine code
when processing a translation unit; rather, it will output the compiler's intermediate representation (e.g. LLVM bitcode).
Later on, when it is time for the linker to run, it will load all of the compiler IR into one giant module, run
optimization passes on the mega-module, and produce a final binary from that.

This works quite well, if all that you're looking for is run-time performance. A major drawback of the LTO approach is
that all of the parallelism gained from optimizing translation units individually is now completely lost; instead, the
linker (using a plugin) will do a single-threaded pass of *all code* produced by compilation steps. This is extremely
slow, memory-intensive, and unable to be run incrementally. There are targets at Meta that simply can't be LTO-compiled
because of their size.

```
flowchart LR;
    header.h;

    header.h --> a.cpp;
    header.h -->|#include| b.cpp;
    header.h --> c.cpp;

    a.cpp --> a.bc;
    b.cpp -->|clang++ -O2 -flto -x ir| b.bc;
    c.cpp --> c.bc;

    a.bc --> a_b_c.bc;
    b.bc -->|linker driver| a_b_c.bc;
    c.bc --> a_b_c.bc;

    a_b_c.bc -->|opt| a_b_c_optimized.bc

    a_b_c_optimized.bc -->|codegen| main.o

    main.o --> |ld| main
```
([Rendered](https://fburl.com/mermaid/kid35io9). `a.bc`, `b.bc`, and `c.bc` are LLVM bitcode; they are all merged
together into a single module, `a_b_c_optimized.bc`, which is then optimized and codegen'd into a final binary.)

The idea of ThinLTO comes from a desire to maintain the ability to optimize modules in parallel while still
allowing for profitable cross-module optimizations.  The idea is this:

1. Just like regular LTO, the compiler emits bitcode instead of machine code. However, it also contains some light
metadata such as a call graph of symbols within the module.
2. The monolithic LTO link is split into three steps: `index`, `opt`, and `link`.

```
flowchart LR;
    header.h;

    header.h --> a.cpp;
    header.h -->|#include| b.cpp;
    header.h --> c.cpp;

    a.cpp --> a.bc;
    b.cpp -->|clang++ -O2 -flto -x ir| b.bc;
    c.cpp --> c.bc;

    a.bc --> index;
    b.bc --> index;
    c.bc --> index;

    index --> a.thinlto.bc;
    index --> b.thinlto.bc;
    index --> c.thinlto.bc;

    a.thinlto.bc --> a.o;
    b.thinlto.bc --> b.o;
    b.bc --> a.o;
    b.bc --> c.o;
    c.thinlto.bc --> c.o;

    a.o --> main;
    b.o -->|ld| main;
    c.o --> main;
```

([Rendered](https://fburl.com/mermaid/56oc99t5))

The `index` step looks like a link step. However, it does not produce a final binary; instead, it looks at every
compiler IR input file that it receives and heuristically determines which other IR modules it should be optimized
with in order to achieve profitable optimizations. These modules might include functions that the index step thinks
probably will get inlined, or globals that are read in the target IR input file. The output of the index step is a
series of files on disk that indicate which sibling object files should be present when optimizing a particular object
file, for each object file in the linker command-line.

The `opt` step runs in parallel for every object file. Each object file will be optimized using the compiler's
optimizer (e.g. `opt`, for LLVM). The optimizer will combine the objects that were referenced as part of the index
step as potentially profitable to include and optimize them all together.

The `link` step takes the outputs of `opt` and links them together, like a normal linker.

In practice, ThinLTO manages to recapture the inherent parallelism of C/C++ compilation by pushing the majority of work
to the parallel `opt` phase of execution. When LLVM performs ThinLTO by default, it will launch a thread pool and process
independent modules in parallel. ThinLTO does not produce as performant a binary as a monolithic LTO; however, in practice,
ThinLTO binaries [paired with AutoFDO](https://fburl.com/wiki/q480euco) perform comparably to monolithic LTO. Furthermore,
ThinLTO's greater efficiency allows for more expensive optimization passes to be run, which can further improve code quality
near that of a monolithic LTO.

This is all great, and ThinLTO has been in use at Meta for some time. However, Buck2 has the ability to take a step
further than Buck1 could ever have - Buck2 can distribute parallel `opt` actions across many machines via Remote Execution
to achieve drastic speedups in ThinLTO wall clock time, memory usage, and incrementality.

## Buck2's Implementation

Buck2's role in a distributed ThinLTO compilation is to construct a graph of actions that directly mirrors the graph
that the `index` step outputs. The graph that the `index` step outputs is entirely dynamic and, as such, the build
system is only aware of what the graph could be after the `index` step is complete. Unlike Buck1 (or even Blaze/Bazel),
Buck2 has explicit support for this paradigm [("dynamic dependencies")](https://fburl.com/gdoc/zklwhkll). Therefore, for Buck2, the basic strategy looks like:

1. Invoke `clang` to act as `index`. `index` will output a file for every object file that indicates what other modules
need to be present when running `opt` on the object file (an "imports file").
2. Read imports files and construct a graph of dynamic `opt` actions whose dependencies mirror the contents of the imports files.
3. Collect the outputs from the `opt` actions and invoke the linker to produce a final binary.

Action `2` is inherently dynamic, since it must read the contents of files produced as part of action `1`. Furthermore,
Buck2's support of `1` is complicated by the fact that certain Buck2 rules can produce an archive of object files as
an output (namely, the Rust compiler). As a result, Buck2's implementation of Distributed ThinLTO is highly dynamic.

Buck2's implementation contains four phases of actions:

1. `thin_lto_prepare`, which specifically handles archives containing LLVM IR and prepares them to be inputs to `thin_lto_index`,
2. `thin_lto_index`, which invokes LLVM's ThinLTO indexer to produce a imports list for every object file to be optimized,
3. `thin_lto_opt`, which optimizes each object file in parallel with its imports present,
4. `thin_lto_link`, which links together the optimized code into a final binary.

### thin_lto_prepare

It is a reality of Buck2 today that some rules don't produce a statically-known list of object files. The list of object
files is known *a priori* during C/C++ compilation, since they have a one-to-one correspondence to source files; however,
the Rust compiler emits an archive of object files; without inspecting the archive, Buck2 has no way of knowing what
the contents of the archive are, or even if they contain bitcode at all.

Future steps (particularly `thin_lto_index`) are defined to only operate on a list of object files - a limitation [inherited from LLVM](https://lists.llvm.org/pipermail/llvm-dev/2019-June/133145.html). Therefore, it is the job of `thin_lto_prepare` to turn an archive into a list of objects - namely, by extracting the archive into a directory.

Buck2 dispatches a `thin_lto_prepare` action for every archive. Each prepare action has two outputs:

1. An **output directory** (called `objects` in the code), a directory that contains the unextracted contents of the archive.
2. A **archive manifest**, a JSON document containing a list of object files that are contained in the output directory.

The core logic of this action is implemented in the Python script `dist_lto_prepare.py`, contained in the `tools` directory. In addition to unpacking each archive, Buck2
keeps track of the list of archives as a Starlark array that will be referenced by index
in later steps.

### thin_lto_index

With all archives prepared, the next step is to invoke LLVM's ThinLTO indexer. For the purposes of Buck2, the indexer
looks like a linker; because of this, Buck2 must construct a reasonable link line. Buck2 does this by iterating over the
list of linkables that it has been given and constructing a link line from them. Uniquely for distributed ThinLTO, Buck2
must wrap all objects that were derived from `thin_lto_prepare` (i.e. were extracted from archives) with `-Wl,--start-lib`
and `-Wl,--end-lib` to ensure that they are still treated as if they were archives by the indexer.

Invoking the indexer is relatively straightforward in that Buck2 invokes it like it would any other linker. However,
once the indexer returns, Buck2 must post-process its output into a format that Buck2's Starlark can understand and
translate into a graph of dynamic `opt` actions. The first thing that Buck2 is write a "meta file" to disk, which
communicates inputs and outputs of `thin_lto_index` to a Python script, `dist_lto_planner.py`. The meta file contains
a list of 7-tuples, whose members are:

1. The path to the source bitcode file. This is used as an index into
    a dictionary that records much of the metadata coming
    from these lines.
2. The path to an output file. `dist_lto_planner.py`is expected to place a
    ThinLTO index file at this location (suffixed `.thinlto.bc`).
3. The path to an output plan. This script is expected to place a link
    plan here (a JSON document indicating which other object files this)
    object file depends on, among other things.
4. If this object file came from an archive, the index of the archive in
    the Starlark archives array.
5. If this object file came from an archive, the name of the archive.
6. If this object file came from an archive, the path to an output plan.
    This script is expected to produce an archive link plan here (a JSON)
    document similar to the object link plan, except containing link
    information for every file in the archive from which this object
    came.
7. If this object file came from an archive, the indexes directory of that
    archive. This script is expected to place all ThinLTO indexes derived
    from object files originating from this archive in that directory.

There are two indices that are derived from this meta file: the object
index (`mapping["index"]`) and the archive index (`mapping["archive_index"]`).
These indices are indices into Starlark arrays for all objects and archive
linkables, respectively. `dist_lto_planner.py` script does not inspect them; rather,
it is expected to communicate these indices back to Starlark by writing them to the
link plan.

`dist_lto_planner.py` reads the index and imports file produced by LLVM and derives
a number of artifacts:

1. For each object file, a `thinlto.bc` file (`bitcode_file`). This file is the same as the input bitcode file, except that LLVM has inserted a number of module imports to refer to the other modules that will be present when the object file is optimized.
2. For each object file, an optimization plan (`plan`). The optimization plan is a JSON document indicating how to construct an `opt` action for this object file. This plan includes
this object file's module imports, whether or not this file contains bitcode at all, a location to place the optimized object file, and a list of archives that this object file imported.
3. For each archive, an optimization plan (`archive_plan`), which contains optimization plans for all of the object files contained within the archive.

This action is a dynamic action because, in the case that there are archives that needed to be preprocessed by `thin_lto_prepare`, this action must read the archive manifest.

### thin_lto_opt

After `thin_lto_index` completes, Buck2 launches `thin_lto_opt` actions for every object file and for every archive. For each object file, Buck2 reads that object file's optimization plan.
At this phase, it is Buck2's responsibility to declare dependencies on every object file referenced by that object's compilation plan; it does so here by adding `hidden` dependencies
on every object file and archive that the archive plan says that this object depends on.

`thin_lto_opt` uses a Python wrapper around LLVM because of a bug (T116695431) where LTO fatal errors don't prevent `clang` from returning an exit code of zero. The Python script wraps
`clang` and exits with a non-zero exit code if `clang` produced an empty object file.

For each archive, Buck2 reads the archive's optimization plan and constructs additional `thin_lto_opt` actions for each object file contained in the archive. Buck2 creates a directory of
symlinks (`opt_objects`) that either contains symlinks to optimized object files (if the object file contained bitcode) or the original object file (if it didn't). The purpose of this symlink directory is to allow the final link to consume object files directly
from this directory without having to know whether they were optimized or not. Paths to these files are passed to the link step
via the optimization manifest (`opt_manifest`).

### thin_lto_link

The final link step. Similar to `thin_lto_index`, this involves creating a link line to feed to the linker that uses the optimized artifacts that we just calculated. In cases where Buck2
would put an archive on the link line, it instead inserts `-Wl,--start-lib`, `-Wl,--end-lib`, and references to the objects in `opt_objects`.

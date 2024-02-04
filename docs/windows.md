# Working on Windows

Jujutsu works the same on all platforms, but there are some caveats that Windows
users should be aware of.

## Line endings are not converted

Jujutsu does not currently honor `.gitattributes` and does not have a setting
like Git's `core.autocrlf`. This means that line endings will be checked out
exactly as they are committed and committed exactly as authored. This is true on
all platforms, but Windows users are most likely to miss CRLF conversion.

If your Git repository expects Windows users to have `core.autocrlf` set to
`true`, then the files are committed with LF line endings but are checked out
with CRLF line endings. Jujutsu doesn't understand this and will convert the
committed line endings to CRLF.

After creating a colocated repository on Windows, you most likely want to set
`core.autocrlf` to `input`, then `jj abandon` to convert all files on disk to LF
line endings:

```powershell
PS> git config core.autocrlf input

# Abandoning the working copy will cause Jujutsu to overwrite all files with
# CRLF line endings with the line endings they are committed with, probably LF
PS> jj abandon
```

This setting ensures Git will check out files with LF line endings without
converting them to CRLF. You'll want to make sure any tooling you use,
especially IDEs, preserve LF line endings.

## Pagination

[Pagination is disabled by default on Windows][issue-2040] because Windows
doesn't ship with a usable pager.

If you have Git installed, you can use Git's pager and re-enable pagination:

```powershell
PS> jj config set --user ui.pager '["C:\\Program Files\\Git\\usr\\bin\\less.exe", "-FRX"]'
PS> jj config set --user ui.paginate auto
```

## Typing `@` in PowerShell

PowerShell uses `@` as part the [array sub-expression operator][array-op], so it
often needs to be escaped or quoted in commands:

```powershell
PS> jj log -r `@
PS> jj log -r '@'
```

One solution is to create a revset alias. For example, to make `HEAD` an alias
for `@`:

```powershell
PS> jj config set --user revset-aliases.HEAD '@'
PS> jj log -r HEAD
```

## WSL sets the execute bit on all files

When viewing a Windows drive from WSL (via _/mnt/c_ or a similar path), Windows
exposes all files with the execute bit set. Since Jujutsu automatically records
changes to the working copy, this sets the execute bit on all files committed in
your repository.

If you only need to access the repository in WSL, the best solution is to clone
the repository in the Linux file system (for example, in
`~/my-repo`).

If you need to use the repository in both WSL and Windows, one solution is to
create a workspace in the Linux file system:

```powershell
PS> jj workspace add --name wsl ~/my-repo
```

Then only use the `~/my-repo` workspace from Linux.

[issue-2040]: https://github.com/martinvonz/jj/issues/2040
[array-op]: https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_arrays?view=powershell-7.4#the-array-sub-expression-operator

## Symbolic link support

`jj` supports symlinks on Windows only when they are enabled by the operating 
system. This requires Windows 10 version 14972 or higher, as well as Developer
Mode. If those conditions are not satisfied, `jj` will materialize symlinks as
ordinary files.

For colocated repositories, Git support must also be enabled using the
`git config` option `core.symlinks=true`.
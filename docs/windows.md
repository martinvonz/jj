# Working on Windows

Jujutsu works the same on all platforms, but there are some caveats that Windows
users should be aware of.

## Line endings are not converted

Jujutsu does not honor `.gitattributes` and does not have a setting like Git's
`core.autocrlf`. This means that line endings will be checked out exactly as
they are committed and committed exactly as authored. This is true on all
platforms, but Windows users are most likely to miss CRLF conversion.

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

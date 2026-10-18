# Luxe's installer for Windows:
#
#     powershell -c "irm https://luxe.dev/install.ps1 | iex"
#
# The installer each release ships refuses to run under Windows' default execution policy, which is
# why its own instructions pass `-ExecutionPolicy ByPass`. This does the same for this PowerShell
# process alone, with nothing saved and no admin needed, then runs the latest release's installer,
# so the address never holds an installer that could drift from what the release built.
#
# A policy set by Group Policy outranks the process scope, as it outranks the command-line flag.
# Setting it then fails, and that is let pass: where the enforced policy allows the installer it runs
# anyway, and where it does not, the installer's own check says so and how to fix it.
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass -Force -ErrorAction SilentlyContinue
Invoke-RestMethod 'https://github.com/evanwhittaker/luxe/releases/latest/download/luxe-cli-installer.ps1' | Invoke-Expression

## Description

Make the 'R' command start R in PowerShell.

PowerShell ships a built-in 'r' alias for 'Invoke-History', and because
aliases take precedence over external commands, typing 'R' reruns the
previous command instead of starting R. This command removes that alias
in your PowerShell profile so that 'R' starts R via rig's quick link.

It edits the 'CurrentUserAllHosts' profile of every PowerShell it finds
(both 'pwsh' and 'powershell'), in an idempotent, clearly marked block.
Start a new PowerShell session for the change to take effect.

Use the '--undo' flag to remove the block rig added, leaving the rest of
the PowerShell profile(s) untouched.

This command does nothing on macOS and Linux.

# Runs a program to its end and answers what it printed and its exit code,
# for the workflows' checks of postrinch.exe. Dot-source it:
#
#   . tools/windows/invoke.ps1
#   $run = Invoke-Program target/release/postrinch.exe --version
#   $run.Code; $run.Out
#
# postrinch.exe is a window (GUI-subsystem) program, and PowerShell does
# not wait for one to exit even when its output is captured: it closed the
# pipe before `postrinch.exe --version` wrote to it ("The pipe is being
# closed", os error 232). A Process with both outputs redirected is read
# to the end and waited for, whatever kind of program it starts.
function Invoke-Program {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(ValueFromRemainingArguments)][string[]]$Arguments = @()
    )
    $info = [System.Diagnostics.ProcessStartInfo]::new((Resolve-Path $Path).Path)
    foreach ($argument in $Arguments) { $info.ArgumentList.Add($argument) }
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = [System.Diagnostics.Process]::Start($info)
    # Both read at once, so neither pipe fills while the other is waited on.
    $err = $process.StandardError.ReadToEndAsync()
    $out = $process.StandardOutput.ReadToEnd()
    $process.WaitForExit()
    $said = [pscustomobject]@{
        Code = $process.ExitCode
        Out  = $out.Trim()
        Err  = $err.Result.Trim()
    }
    if ($said.Err) { Write-Host $said.Err }
    $said
}

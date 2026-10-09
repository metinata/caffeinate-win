# Installs caffeinate for Windows for the current user:
#
#     irm https://github.com/metinata/caffeinate-win/releases/latest/download/install.ps1 | iex
#
# Downloads the latest release for this machine (x64 or ARM64), checks its
# SHA-256 checksum, puts it in %LOCALAPPDATA%\Programs\caffeinate and adds that
# folder to the user's PATH. No administrator rights are needed. Run it again to
# update.

# A script block of its own, so that nothing leaks into the session that runs `iex`.
& {
    $ErrorActionPreference = 'Stop'
    # Windows PowerShell downloads much faster without a progress bar.
    $ProgressPreference = 'SilentlyContinue'
    # Older Windows PowerShell setups do not turn on TLS 1.2, which GitHub requires.
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $release = 'https://github.com/metinata/caffeinate-win/releases/latest/download'
    $dir = Join-Path $env:LOCALAPPDATA 'Programs\caffeinate'

    # A 32-bit PowerShell sees the machine's architecture in PROCESSOR_ARCHITEW6432.
    $machine = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    $arch = switch ($machine) {
        'AMD64' { 'x86_64' }
        'ARM64' { 'aarch64' }
        default { throw "caffeinate needs 64-bit Windows on x64 or ARM64, not $machine." }
    }
    $name = "caffeinate-$arch-pc-windows-msvc.zip"

    $temp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
    New-Item -ItemType Directory -Path $temp | Out-Null
    try {
        Write-Host "Downloading $name"
        $zip = Join-Path $temp $name
        Invoke-WebRequest -Uri "$release/$name" -OutFile $zip -UseBasicParsing
        Invoke-WebRequest -Uri "$release/$name.sha256" -OutFile "$zip.sha256" -UseBasicParsing
        $expected = (Get-Content -Path "$zip.sha256" -Raw).Split(' ')[0].Trim()
        $actual = (Get-FileHash -Path $zip -Algorithm SHA256).Hash
        if ($actual -ne $expected) {
            throw "The download is damaged: its SHA-256 is $actual, but the release says $expected."
        }

        Expand-Archive -Path $zip -DestinationPath (Join-Path $temp 'files')
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        try {
            Copy-Item -Path (Join-Path $temp 'files\*') -Destination $dir -Force
        } catch {
            throw "Cannot replace the files in ${dir}: $($_.Exception.Message) Stop caffeinate if it is running, and try again."
        }
    } finally {
        Remove-Item -Path $temp -Recurse -Force -ErrorAction SilentlyContinue
    }

    # Add the folder to the user's PATH. The registry value is read and written
    # as is, so that entries such as %USERPROFILE%\... keep their variables.
    $environment = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    try {
        $path = $environment.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
        $added = ($path -split ';') -notcontains $dir
        if ($added) {
            $environment.SetValue('Path', ($path.TrimEnd(';') + ";$dir").TrimStart(';'), 'ExpandString')
        }
    } finally {
        $environment.Close()
    }
    if ($added) {
        $env:Path = "$($env:Path.TrimEnd(';'));$dir"
        # Tell Windows that the environment changed, so that terminals opened
        # from now on find caffeinate too.
        try {
            Add-Type -Namespace CaffeinateInstaller -Name Win32 -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint msg, UIntPtr wParam, string lParam, uint flags, uint timeout, out UIntPtr result);
'@
            $result = [UIntPtr]::Zero
            $HWND_BROADCAST = [IntPtr]0xffff; $WM_SETTINGCHANGE = 0x1A; $SMTO_ABORTIFHUNG = 2
            [void][CaffeinateInstaller.Win32]::SendMessageTimeout($HWND_BROADCAST, $WM_SETTINGCHANGE, [UIntPtr]::Zero, 'Environment', $SMTO_ABORTIFHUNG, 5000, [ref]$result)
            $told = $true
        } catch {
            $told = $false
        }
    }

    $version = & (Join-Path $dir 'caffeinate.exe') --version
    Write-Host "Installed $version in $dir"
    if ($added) {
        Write-Host "Added $dir to your PATH."
        if (-not $told) {
            Write-Host 'Other terminals find caffeinate after you sign out and in again.'
        }
    }
    Write-Host 'Run "caffeinate -h" to get started.'
}

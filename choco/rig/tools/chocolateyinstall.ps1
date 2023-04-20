$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.5.2/rig-windows-0.5.2.exe'
  Checksum64     = '041bc4aceb0aeda2e3687adffe24b9aa1e12a2849c820ebdc13049941c7c9aa4'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

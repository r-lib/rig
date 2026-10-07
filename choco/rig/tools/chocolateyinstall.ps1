$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.11.0/rig-windows-0.11.0.exe'
  Checksum64     = 'eb6d0da7755230533ea0c809ab7dcff16c1df710d46529a00c61c2c11d6e70de'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

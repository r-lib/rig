$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.5.3/rig-windows-0.5.3.exe'
  Checksum64     = 'dfc63ae036fc02ee8719515ba600cdf4d758d4707cdecfb283c65f02f0b07163'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

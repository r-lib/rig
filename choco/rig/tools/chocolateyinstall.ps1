$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.8.1/rig-windows-0.8.1.exe'
  Checksum64     = '586d225f9213bcbe61e452e56338d8109ee99fef479efd51e3d23ff7fc16cfd3'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

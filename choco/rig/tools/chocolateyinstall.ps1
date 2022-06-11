$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.4.1/rig-windows-0.4.1.exe'
  Checksum64     = '6cd9b2f725f9d426551abbebafd13634f47245b2703f2b83652b11f8fd5f3a47'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

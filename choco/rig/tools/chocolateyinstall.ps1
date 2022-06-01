$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.4.0/rig-windows-0.4.0.exe'
  Checksum64     = '2080616af027d80f1d84e8d2ecf1a24b2e3afd624d2604832fe01ec724aac95d'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.6.0/rig-windows-0.6.0.exe'
  Checksum64     = '8b75b5a912800e3d0c2ef2470d7633cd8c329d2cf5451c9f8f98dc3a965a6b8e'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

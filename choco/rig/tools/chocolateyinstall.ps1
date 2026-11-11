$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.11.1/rig-windows-0.11.1.exe'
  Checksum64     = '695a6c34357635ce2436efcc3ea5ff7264825294578c62ac3683efd6c106ab68'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

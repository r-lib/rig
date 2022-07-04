$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.5.0/rig-windows-0.5.0.exe'
  Checksum64     = '2aff8ba53f0e4a3fe56be7c2161049e6c4e2f707d2bb3ba9d5781be3ad75b016'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

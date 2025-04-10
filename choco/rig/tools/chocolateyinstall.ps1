$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.7.0/rig-windows-0.7.0.exe'
  Checksum64     = '9e4d8bae8ce820d25b9a1f49a54ce9c2fa732bcccd47920f49d714f21beb1e02'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

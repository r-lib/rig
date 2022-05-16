$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.3.0/rig-windows-0.3.0.exe'
  Checksum64     = 'dd72690fa9eac10e43e6c7cf96b4387de0fd40cb2456668d7f75f34656d3d3af'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

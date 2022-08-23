$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.5.1/rig-windows-0.5.1.exe'
  Checksum64     = '701a92776d9857de83cb8c9f472ed6a227b823297b929495de1cb48951fb86aa'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

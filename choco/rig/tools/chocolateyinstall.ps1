$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.8.0/rig-windows-0.8.0.exe'
  Checksum64     = '1de99a3906453c2e18848a587cfd752183e9cfaa2b7931f60c64c40f04489eab'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

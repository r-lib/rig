$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.8.0-beta/rig-windows-0.8.0-beta.exe'
  Checksum64     = '23746671fbea4fdd66e859b6a47da8aa514009cb925b3e9743d77a3ce55503c7'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

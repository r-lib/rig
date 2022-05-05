$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rim*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rim/releases/download/v0.2.3/rim-windows-0.2.3.exe'
  Checksum64     = 'ae03f18e825af8ced9a4d3058f09d9ad6f7e4824901b9e6808d9874a334bdd74'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

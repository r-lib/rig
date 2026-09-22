$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.10.0/rig-windows-0.10.0.exe'
  Checksum64     = '8934534920bf55ddef3215209e7c940445b933d39350260bf00932c2bd6e9338'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$packageArgs = @{
  softwareName   = 'rig*'
  PackageName    = $env:ChocolateyPackageName
  FileType       = 'exe'
  SilentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES'
  Url64bit       = 'https://github.com/r-lib/rig/releases/download/v0.7.1/rig-windows-0.7.1.exe'
  Checksum64     = '44a8bbe4fc8c6225c7eabe9b7110247b7cac8a5b53b7326e61e50b443a6aa88c'
  ChecksumType64 = 'sha256'
}

Install-ChocolateyPackage @packageArgs

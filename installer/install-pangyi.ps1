# 旁译 输入法 安装脚本 —— 严格照青简（qingjian）卸载/安装的做法：
#   1) 装到 Program Files\Pangyi（需要管理员，和青简的安装器一样）
#   2) 给目录加 ALL APPLICATION PACKAGES 读+执行权限（UWP/打包应用，比如新版记事本，才能加载 DLL）
#   3) 注册 64 位与 32 位各一份（HKLM 与 WOW6432Node），注册表结构与青简完全一致
#   4) 图标放到 ProgramData\Pangyi，语言配置里写 Description=旁译
$ErrorActionPreference = 'Stop'
$log = 'C:\ProgramData\Pangyi\install.log'
New-Item -ItemType Directory -Force -Path 'C:\ProgramData\Pangyi' | Out-Null
function W($m) { $m | Out-File -FilePath $log -Append -Encoding UTF8; Write-Host $m }
Remove-Item $log -Force -ErrorAction SilentlyContinue

$src   = 'C:\Users\33354\Documents\Codex\2026-10-08\github-yenharvey-qingjian\pangyi\dist'
$dst   = 'C:\Program Files\Pangyi'
$dll64 = 'pangyi_ime-0.1.0.dll'
$dll32 = 'pangyi_ime-0.1.0-x86.dll'
$ico   = 'pangyi.ico'
$clsid = '{B4E8F2A1-7C3D-4E5F-9A21-8D6C4B3E1F07}'
$prof  = '{5A7C9E12-3D4B-4F8A-B6E1-2C9D7A5E8F34}'
$cats  = @(
 '{046B8C80-1647-40F7-9B21-B93B81AABC1B}','{13A016DF-560B-46CD-947A-4C3AF1E0E35D}',
 '{25504FB4-7BAB-4BC1-9C69-CF81890F0EF5}','{34745C63-B2F0-4784-8B67-5E12C8701A31}',
 '{364215D9-75BC-11D7-A6EF-00065B84435C}','{49D2F9CE-1F5E-11D7-A6D3-00065B84435C}',
 '{49D2F9CF-1F5E-11D7-A6D3-00065B84435C}','{CCF05DD7-4A87-11D7-A6E2-00065B84435C}'
)
W '=== 旁译 安装 开始 ==='
W ('管理员: ' + ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))

# 1) 文件
New-Item -ItemType Directory -Force -Path $dst | Out-Null
Copy-Item (Join-Path $src $dll64) (Join-Path $dst $dll64) -Force
Copy-Item (Join-Path $src $dll32) (Join-Path $dst $dll32) -Force
Copy-Item (Join-Path $src $ico)   (Join-Path $dst $ico)   -Force
Copy-Item (Join-Path $src $ico)   'C:\ProgramData\Pangyi\pangyi.ico' -Force
W ('已安装 64 位: ' + (Join-Path $dst $dll64))
W ('已安装 32 位: ' + (Join-Path $dst $dll32))

# 2) 打包应用权限
$ic = & icacls.exe $dst /grant '*S-1-15-2-1:(OI)(CI)RX' /T /C /Q 2>&1
W ('icacls: ' + ($ic -join ' | '))

# 3) 注册（HKLM 64 位 + WOW6432Node 32 位）
function Reg-Clsid($root, $serverPath) {
  $k = "$root\$clsid"
  New-Item -Force -Path $k | Out-Null
  Set-ItemProperty -Path $k -Name '(default)' -Value '旁译'
  New-Item -Force -Path "$k\InprocServer32" | Out-Null
  Set-ItemProperty -Path "$k\InprocServer32" -Name '(default)' -Value $serverPath
  Set-ItemProperty -Path "$k\InprocServer32" -Name 'ThreadingModel' -Value 'Apartment'
}
Reg-Clsid 'HKLM:\SOFTWARE\Classes\CLSID' (Join-Path $dst $dll64)
Reg-Clsid 'HKLM:\SOFTWARE\WOW6432Node\Classes\CLSID' (Join-Path $dst $dll32)
W '已注册 CLSID（64 位 + 32 位）'

# 4) TSF 文本服务 + 语言配置
$tip = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$clsid"
New-Item -Force -Path $tip | Out-Null
Set-ItemProperty -Path $tip -Name 'Enable' -Value 1 -Type DWord
foreach ($c in $cats) {
  New-Item -Force -Path "$tip\Category\Category\$c\$clsid" | Out-Null
  New-Item -Force -Path "$tip\Category\Item\$clsid\$c" | Out-Null
}
$lp = "$tip\LanguageProfile\0x00000804\$prof"
New-Item -Force -Path $lp | Out-Null
Set-ItemProperty -Path $lp -Name 'Description' -Value '旁译'
Set-ItemProperty -Path $lp -Name 'IconFile' -Value 'C:\ProgramData\Pangyi\pangyi.ico'
Set-ItemProperty -Path $lp -Name 'IconIndex' -Value 0 -Type DWord
W '已注册 TSF 文本服务 + 语言配置'

# 5) 清掉旧的使用者级注册（HKCU 优先于 HKLM，留着会指向旧路径）
foreach ($h in @("HKCU:\Software\Classes\CLSID\$clsid", "HKCU:\Software\Microsoft\CTF\TIP\$clsid")) {
  if (Test-Path $h) { Remove-Item -Recurse -Force $h; W ('已清除旧项: ' + $h) }
}
W '=== 完成 ==='
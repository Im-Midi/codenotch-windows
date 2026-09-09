param([Parameter(Mandatory)][int]$AppProcessId)
$ErrorActionPreference='Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class NotchWindowCheck {
 public delegate bool Callback(IntPtr h,IntPtr p);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h,StringBuilder b,int c);
 [DllImport("user32.dll")] static extern bool EnumWindows(Callback cb,IntPtr p);
 [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h,out uint id);
 [DllImport("user32.dll",EntryPoint="GetWindowLongPtrW")] public static extern IntPtr Style(IntPtr h,int n);
 [DllImport("user32.dll")] public static extern int GetWindowRgn(IntPtr h,IntPtr r);
 [DllImport("gdi32.dll")] public static extern IntPtr CreateRectRgn(int x,int y,int r,int b);
 [DllImport("gdi32.dll")] public static extern bool PtInRegion(IntPtr r,int x,int y);
 [DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr r);
 public static IntPtr Find(uint pid) {IntPtr found=IntPtr.Zero;EnumWindows((h,p)=>{uint id;GetWindowThreadProcessId(h,out id);var title=new StringBuilder(256);GetWindowText(h,title,256);if(id==pid && title.ToString()=="Codenotch")found=h;return true;},IntPtr.Zero);return found;}
}
'@
$handle=[NotchWindowCheck]::Find($AppProcessId)
if ($handle -eq [IntPtr]::Zero) { throw 'Passive notch window not found.' }
$style=[NotchWindowCheck]::Style($handle,-20).ToInt64()
$region=[NotchWindowCheck]::CreateRectRgn(0,0,0,0)
try {
 $regionType=[NotchWindowCheck]::GetWindowRgn($handle,$region)
 $layout=Get-Content (Join-Path $PSScriptRoot '../output/live-layout-check.json') | ConvertFrom-Json
 $x=[int](($layout.pill.x+$layout.pill.width/2)*$layout.dpr)
 $y=[int](($layout.pill.y+$layout.pill.height/2)*$layout.dpr)
 $result=[ordered]@{NoActivate=($style -band 0x08000000) -ne 0;ToolWindow=($style -band 0x80) -ne 0;TransparentCornerPassesThrough=-not [NotchWindowCheck]::PtInRegion($region,5,5);PillReceivesInput=[NotchWindowCheck]::PtInRegion($region,$x,$y);NativeRegionType=$regionType}
 if (-not $result.NoActivate -or -not $result.ToolWindow -or -not $result.TransparentCornerPassesThrough -or -not $result.PillReceivesInput) { throw ($result|ConvertTo-Json -Compress) }
 $result|ConvertTo-Json|Set-Content (Join-Path $PSScriptRoot '../output/native-window-check.json')
 Write-Output 'PASS: native no-activate/tool-window flags, transparent corner excluded, pill input region included.'
} finally { [void][NotchWindowCheck]::DeleteObject($region) }

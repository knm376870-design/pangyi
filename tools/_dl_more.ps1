$ProgressPreference='SilentlyContinue'
$base='C:\Users\33354\Documents\Codex\2026-10-08\github-yenharvey-qingjian\pangyi\data\raw'
$list=@(
 @('zhwikibooks','https://dumps.wikimedia.org/zhwikibooks/latest/zhwikibooks-latest-pages-articles.xml.bz2'),
 @('zhwikiversity','https://dumps.wikimedia.org/zhwikiversity/latest/zhwikiversity-latest-pages-articles.xml.bz2')
)
foreach ($it in $list) {
  $out = Join-Path $base ($it[0] + '.xml.bz2')
  if (Test-Path $out) { continue }
  Invoke-WebRequest -Uri $it[1] -OutFile $out -TimeoutSec 3600 -UseBasicParsing
}
'ok' | Out-File (Join-Path $base 'dumps2-done.txt') -Encoding UTF8

#!/usr/bin/env python3
"""Build a self-contained, credential-free Observatory recording."""
import argparse,json,re
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('source',type=Path);p.add_argument('output',type=Path);p.add_argument('--recording',type=Path);a=p.parse_args()
assets={k:(a.source/n).read_text() for k,n in [('css','observatory.css'),('js','observatory.js'),('metrics','metrics.mjs')]}
data=json.loads((a.recording or a.source/'recording.json').read_text())
encoded=lambda x:json.dumps(x,ensure_ascii=False,separators=(',',':')).replace('<','\\u003c')
script='window.__OBSERVATORY_RECORDING__='+encoded(data)+';\nwindow.__OBSERVATORY_ASSETS__='+encoded(assets)+';\n'+assets['metrics'].replace('export function','function')+'\n'+re.sub(r"^import[\s\S]*?from\s*['\"]\./metrics\.mjs['\"];?\s*",'',assets['js'])
html=(a.source/'index.html').read_text().replace('<link rel="stylesheet" href="observatory.css">','<style>'+assets['css']+'</style>').replace('<script type="module" src="observatory.js"></script>','<script type="module">'+script+'</script>')
a.output.write_text(html);print(str(a.output),a.output.stat().st_size)

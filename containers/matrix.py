#!/usr/bin/env python3

import json
import argparse

parser = parser = argparse.ArgumentParser(description="Print containers as JSON.")
parser.add_argument(
    'containers',
    type = str,
    nargs = '?',
    default = 'all',
    help = 'Comma-separated list of container types. Default is "all" to use all containers.'
)
args = parser.parse_args()

f = open('containers/matrix.json')
data = json.load(f)
f.close()

if args.containers == 'all':
    print(json.dumps(data))
else:
    res=[]
    sel = args.containers.split(",")
    sel = [ s.strip() for s in sel ]
    for s in sel:
        for i, cnt in enumerate(data):
            if s in cnt['aliases']:
                res.append(i)
    res = list(set(res))
    data = [ d for i, d in enumerate(data) if i in res ]
    print(json.dumps(data))

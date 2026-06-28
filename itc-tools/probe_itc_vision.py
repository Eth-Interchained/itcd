#!/usr/bin/env python3
# ─────────────────────────────────────────────────────────────────────────────
# INTERCHAINED · itcd × NEDB  —  on-chain proof probe
# Probes vision.interchained.org and reconciles each tx to the satoshi.
# Pure stdlib (urllib) — runs anywhere python3 lives (macOS / MinGW / Linux).
#
#   python3 probe_itc.py                      # today's round-trip (2 txs)
#   python3 probe_itc.py <txid> [<txid> ...]  # probe whatever you pass
# ─────────────────────────────────────────────────────────────────────────────
import json, sys, urllib.request

API = "https://vision.interchained.org/api"

# Default when no txids are passed: today's round-trip on the NEDB-backed itcd.
DEFAULT_TXS = [
    "66243635bb004bb278f649935d45dfef939a80295e26b5d99c66d94dc8176a12",
    "0fc446ce682a80ee8759aa7424d8c7777cb7dde3946b317dd51776fbf39f78b7",
]

C = dict(r="\033[0m", b="\033[1m", d="\033[2m", g="\033[32m", c="\033[36m", y="\033[33m", m="\033[35m", red="\033[31m")
def itc(s):  return f"{s/1e8:.8f}"
def bar(ch="─", n=68): return ch * n
def get(path):
    req = urllib.request.Request(f"{API}/{path}", headers={"User-Agent": "interchained-probe"})
    with urllib.request.urlopen(req, timeout=45) as r:
        return json.loads(r.read().decode())

argv = sys.argv[1:]
if any(a in ("-h", "--help") for a in argv):
    print("usage: python3 probe_itc.py [<txid> ...]   (no args = today's round-trip)")
    sys.exit(0)
txids = argv or DEFAULT_TXS
mode  = "custom" if argv else "today's round-trip"

print(f"\n{C['c']}{C['b']}╔{bar('═')}╗{C['r']}")
print(f"{C['c']}{C['b']}║  INTERCHAINED · itcd × NEDB — on-chain proof probe" + " " * 17 + f"║{C['r']}")
print(f"{C['c']}{C['b']}╚{bar('═')}╝{C['r']}")
print(f"{C['d']}  probing {len(txids)} tx ({mode})  ·  {API}{C['r']}")

all_ok = True
seen = []   # ordered-unique addresses touched, for "follow the money"
for n, txid in enumerate(txids, 1):
    print(f"\n{C['m']}{C['b']}▶ TX-{n}{C['r']}  {C['d']}{txid}{C['r']}")
    try:
        tx = get(f"tx/{txid}")
    except Exception as e:
        print(f"{C['red']}  probe failed: {e}{C['r']}"); all_ok = False; continue
    ins  = sum((i.get("prevout") or {}).get("value_sats", 0) for i in tx["inputs"])
    outs = sum(o["value_sats"] for o in tx["outputs"])
    fee  = tx.get("fee_sats", 0); conf = tx.get("confirmations", 0)
    state = f"{C['g']}confirmed x{conf}{C['r']}" if conf and not tx.get("in_mempool") else f"{C['y']}in mempool{C['r']}"
    print(f"  status   {state}   {C['d']}fee {fee} sat @ {tx.get('fee_rate_sat_vbyte','?')} sat/vB{C['r']}")
    for i in tx["inputs"]:
        pv = i.get("prevout")
        if not pv:
            print(f"  {C['y']}in {C['r']}{'(coinbase)':>17}      {C['d']}newly minted{C['r']}"); continue
        seen.append(pv["address"])
        print(f"  {C['y']}in {C['r']}{itc(pv['value_sats']):>17} ITC  {C['d']}{pv['address']}{C['r']}")
    for o in tx["outputs"]:
        a = o.get("address") or "(non-address script)"
        if a.startswith("itc"): seen.append(a)
        print(f"  {C['g']}out{C['r']}{itc(o['value_sats']):>17} ITC  {C['d']}{a}{C['r']}")
    if tx.get("is_coinbase"):
        print(f"  {C['b']}⛏  coinbase — minted {itc(outs)} ITC (subsidy + fees){C['r']}")
    else:
        ok = (ins == outs + fee)
        mark = f"{C['g']}✓ reconciled to the satoshi{C['r']}" if ok else f"{C['red']}✗ MISMATCH{C['r']}"
        print(f"  {C['b']}Σ in {itc(ins)} = out {itc(outs)} + fee {itc(fee)}  {mark}{C['r']}")
        all_ok = all_ok and ok

uniq = list(dict.fromkeys(seen))
if uniq:
    print(f"\n{C['m']}{C['b']}▶ follow the money — {len(uniq)} address(es) touched{C['r']}")
    for addr in uniq:
        try:
            a = get(f"address/{addr}")
            print(f"  {C['c']}{itc(a['balance']['confirmed_sats']):>17} ITC{C['r']}  {C['d']}{addr}  ({a['tx_count']} tx){C['r']}")
        except Exception as e:
            print(f"  {C['red']}addr probe failed for {addr}: {e}{C['r']}")

print(f"\n{C['c']}{bar('─')}{C['r']}")
tag = "ALL TRANSACTIONS RECONCILED — NEDB-backed itcd, live on-chain." if all_ok else "CHECK FAILED — investigate above."
print(f"{C['b']}{C['g'] if all_ok else C['red']}  {tag}{C['r']}")
print(f"{C['d']}  3>1 — M + Vex + the Oracle.  Lightning strikes, thunder roars, code appears.{C['r']}\n")

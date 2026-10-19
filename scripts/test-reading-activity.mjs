// Real Pinia activity store, deterministic clocks and local IPC/event mocks only.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createPinia, defineStore } from 'pinia'
import { computed, ref } from 'vue'
import ts from 'typescript'
const source=await readFile(new URL('../src/stores/activity.ts',import.meta.url),'utf8')
const script=source.replace(/^import .*\n/gm,'').replace(/^export /gm,'')
const code=ts.transpileModule(script,{compilerOptions:{target:ts.ScriptTarget.ES2021,module:ts.ModuleKind.None}}).outputText
const clone=x=>JSON.parse(JSON.stringify(x)), flush=async()=>{for(let i=0;i<20;i++)await Promise.resolve()}
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return{promise,resolve}}
function environment(){
  let now=new Date('2026-10-19T09:00:00+08:00').getTime()
  const disk=new Map(),cache=new Map(),listeners=new Set(),saves=[]
  const clock=class extends Date{constructor(...args){super(...(args.length?args:[now]))}static now(){return now}}
  const get=root=>clone(disk.get(root)||{version:2,days:{}})
  let hold=null
  const invoke=async(name,args)=>{
    if(name==='get_device_id')return'machine'
    if(name==='get_activity_log')return hold?.root===args.root?hold.response.promise:get(args.root)
    if(name==='save_activity_log'){
      const value=get(args.root)
      for(const[date,day]of Object.entries(args.data.days)){
        value.days[date]??={date,devices:{}}
        if(day.devices[args.deviceId])value.days[date].devices[args.deviceId]=clone(day.devices[args.deviceId])
      }
      disk.set(args.root,value);saves.push(clone(args))
      for(const callback of listeners)callback({payload:{root:args.root,writer:args.deviceId}})
      return
    }
    throw Error(name)
  }
  const make=()=>{
    const timers=new Set()
    const factory=new Function('defineStore','computed','ref','invoke','listen','localStorage','Date','setInterval','clearInterval',code+'; return useActivityStore')
    const useStore=factory(defineStore,computed,ref,invoke,async(_name,callback)=>{listeners.add(callback);return()=>listeners.delete(callback)},
      {getItem:k=>cache.get(k)??null,setItem:(k,v)=>cache.set(k,v)},clock,callback=>{timers.add(callback);return callback},callback=>timers.delete(callback))
    return {store:useStore(createPinia()),tick:()=>{for(const cb of timers)cb()}}
  }
  return{make,disk,saves,cache,advance:ms=>now+=ms,get,
    hold(root){const response=deferred();hold={root,response};return response},unhold(){hold=null}}
}
const total=(data,slug)=>Object.values(data.days).flatMap(day=>Object.values(day.devices)).reduce((n,slot)=>n+(slug?slot.paper_reading_ms?.[slug]?.duration_ms??0:slot.reading_ms??0),0)

test('main PDF/sidebar and standalone handoffs add reading time once and stale saves preserve both writers',async()=>{
  const env=environment(),main=env.make(),popup=env.make()
  main.store.startHeartbeat();popup.store.startHeartbeat()
  await main.store.load('library');await popup.store.load('library','paper-ai-paper')
  main.store.beginReading('paper','Paper');env.advance(60000);main.store.endReading();await flush()
  popup.store.beginReading('paper','Paper');env.advance(90000);popup.tick();await flush()
  assert.equal(total(env.get('library'),'paper'),150000)
  assert.equal(total(main.store.data,'paper'),150000,'peer save refreshes the visible activity statistics')
  popup.store.endReading();await flush()
  main.store.beginReading('paper','Paper');env.advance(30000);main.store.endReading();await flush()
  assert.equal(total(env.get('library'),'paper'),180000)
  const writers=Object.keys(Object.values(env.get('library').days)[0].devices)
  assert.deepEqual(writers.sort(),['machine','machine:window:paper-ai-paper'].sort())
  env.advance(60000);main.tick();popup.tick();await flush()
  assert.equal(total(env.get('library'),'paper'),180000,'background generation/inactive windows add no time')
  main.store.stopHeartbeat();popup.store.stopHeartbeat()
})

test('two standalone papers keep their own times and reopening a popup resumes its existing cell',async()=>{
  const env=environment(),one=env.make(),two=env.make()
  await one.store.load('library','paper-ai-one');await two.store.load('library','paper-ai-two')
  one.store.beginReading('one','First');env.advance(45000);one.store.endReading();await flush()
  two.store.beginReading('two','Second');env.advance(75000);two.store.endReading();await flush()
  const reopened=env.make();await reopened.store.load('library','paper-ai-one')
  reopened.store.beginReading('one','First');env.advance(15000);reopened.store.endReading();await flush()
  assert.equal(total(env.get('library'),'one'),60000)
  assert.equal(total(env.get('library'),'two'),75000)
  assert.equal(total(env.get('library')),135000)
})

test('switching libraries flushes the old root and rejects delayed loads/reloads',async()=>{
  const env=environment(),window=env.make()
  await window.store.load('old','paper-ai-one')
  window.store.beginReading('one','First');env.advance(12000)
  await window.store.load('new','paper-ai-one');await flush()
  assert.equal(total(env.get('old')),12000);assert.equal(total(env.get('new')),0)
  const pending=env.hold('slow'),oldLoad=window.store.load('slow','paper-ai-one');await flush()
  await window.store.load('new','paper-ai-one')
  pending.resolve({version:2,days:{}});await oldLoad
  assert.equal(window.store.libraryPath,'new')
  window.store.beginReading('two','Second');env.advance(8000);window.store.endReading();await flush()
  assert.equal(total(env.get('new'),'two'),8000)
})

test('popup fallback cache cannot migrate or duplicate the main window history',async()=>{
  const env=environment(),main=env.make(),popup=env.make()
  await main.store.load('library');main.store.beginReading('paper','Paper');env.advance(10000);main.store.endReading();await flush()
  env.disk.clear()
  await popup.store.load('library','paper-ai-paper')
  assert.equal(total(popup.store.data),0)
  popup.store.beginReading('paper','Paper');env.advance(5000);popup.store.endReading();await flush()
  assert(env.cache.has('argus:activity:library:window:paper-ai-paper'))
  assert.equal(total(env.get('library')),5000)
})


test('a delayed peer refresh cannot roll activity statistics back within the same library',async()=>{
  const env=environment(),main=env.make(),popup=env.make()
  await main.store.load('library');await popup.store.load('library','paper-ai-one')
  main.store.beginReading('one','First');env.advance(5000);main.store.endReading();await flush()
  const old=env.get('library'),held=env.hold('library'),pending=main.store.reloadFromDisk();await flush();env.unhold()
  popup.store.beginReading('one','First');env.advance(10000);popup.store.endReading();await flush()
  await main.store.reloadFromDisk();assert.equal(total(main.store.data),15000)
  held.resolve(old);await pending;assert.equal(total(main.store.data),15000)
})

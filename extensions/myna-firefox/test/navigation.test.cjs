"use strict";
const test=require("node:test");
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");
const script=fs.readFileSync(path.join(__dirname,"..","navigation.js"),"utf8");

function setup() {
  let listener, entries=[], clicks=0, current="https://example.org/page";
  const observing=new Set();
  class Observer {
    constructor(cb) {this.cb=cb;}
    observe(){observing.add(this);}
    disconnect(){observing.delete(this);}
  }
  const mk=(text,url="/doc",options={})=>({
    tagName:options.tagName||"A",
    isConnected:true,innerText:text,textContent:text,
    getAttribute(name){
      if(name==="href")return url;
      if(name==="type")return options.type??null;
      return null;
    },
    hasAttribute(name){return name==="download" && Boolean(options.download);},
    closest(query){return query==="form" && options.form?{}:null;},
    matches(){return false;},
    getBoundingClientRect(){return {x:0,y:0,width:120,height:30};},
    click(){clicks++;}
  });
  const context={
    browser:{runtime:{onMessage:{addListener(fn){listener=fn;}}}},
    document:{documentElement:{},visibilityState:"visible",querySelectorAll(){return entries;}},
    location:{href:current,origin:"https://example.org"},
    window:{scrollBy(){}},URL,setTimeout,clearTimeout,MutationObserver:Observer,
    getComputedStyle(){return {display:"block",visibility:"visible",opacity:"1"};},
    addEventListener(){},innerHeight:800,
  };
  vm.runInNewContext(script,context,{timeout:2000});
  return {
    mk,put(...newEntries){entries=newEntries;for(const observer of [...observing])observer.cb([]);},
    request(action){return listener({type:"myna.nav.execute",version:1,action});},
    get clicks(){return clicks;}
  };
}

test("lists labeled elements and clicks a recent number",async()=>{
  const h=setup();
  h.put(h.mk("Documentation"));
  const list=await h.request({type:"list"});
  assert.equal(list.ok,true);
  assert.equal(list.items[0].label,"Documentation");
  const result=await h.request({type:"click",target:{kind:"index",index:1}});
  assert.equal(result.ok,true);
  assert.equal(h.clicks,1);
});

test("blocks external links and submitting buttons",async()=>{
  const h=setup();
  h.put(h.mk("Another domain","https://other.test/"));
  await h.request({type:"list"});
  assert.equal((await h.request({type:"click",target:{kind:"index",index:1}})).code,
    "CROSS_SITE_REQUIRES_CONFIRMATION");
  h.put(h.mk("Continuer",null,{tagName:"BUTTON",form:true}));
  assert.equal((await h.request({type:"click",target:{kind:"label",role:"button",label:"continuer"}})).code,
    "FORM_SUBMISSION_BLOCKED");
  assert.equal(h.clicks,0);
});

test("waits for an asynchronously arriving element",async()=>{
  const h=setup();
  const waiting=h.request({type:"wait",target:{kind:"label",role:"link",label:"continuer"},timeout_ms:800});
  setTimeout(()=>h.put(h.mk("Continuer")),20);
  const result=await waiting;
  assert.equal(result.ok,true);
  assert.equal(result.action,"ready");
  assert.equal(h.clicks,0);
});

test("cancels a pending wait",async()=>{
  const h=setup();
  const waiting=h.request({type:"wait",target:{kind:"label",role:"link",label:"continuer"},timeout_ms:800});
  await h.request({type:"cancel"});
  const result=await waiting;
  assert.equal(result.ok,false);
  assert.equal(result.code,"CANCELLED");
});

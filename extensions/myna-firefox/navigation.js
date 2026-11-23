/* Myna R1: page-scoped DOM actions; no page source or arbitrary JS leaves Firefox. */
(() => {
  "use strict";
  if (globalThis.__mynaNavInstalledV1) return;
  globalThis.__mynaNavInstalledV1 = true;

  const MAX_ITEMS = 60;
  const MAX_WAIT = 12000;
  let snapshot = [];
  let listedAt = 0;
  let listedUrl = "";
  let waiting = null;

  const fold = text => String(text || "").normalize("NFD").replace(/[\u0300-\u036f]/g,"")
    .toLocaleLowerCase("fr").replace(/\s+/g," ").trim();
  function label(el) {
    return (el.getAttribute("aria-label") || el.getAttribute("title") ||
      el.getAttribute("alt") || el.innerText || el.textContent ||
      el.getAttribute("value") || "").trim().replace(/\s+/g," ").slice(0,100);
  }
  const role = el => el.tagName==="A"?"link":"button";
  function visible(el) {
    if (!el.isConnected || el.closest("[hidden],[inert],[aria-hidden='true']")) return false;
    if (el.matches(":disabled") || el.getAttribute("aria-disabled")==="true") return false;
    const style = getComputedStyle(el), rect=el.getBoundingClientRect();
    return style.display!=="none" && style.visibility!=="hidden" &&
      Number(style.opacity)>0 && rect.width>0 && rect.height>0;
  }
  function interactive() {
    return Array.from(document.querySelectorAll(
      "a[href],button,[role='button'],input[type='button'],input[type='submit']"
    )).filter(el=>visible(el) && label(el).length>0).slice(0,MAX_ITEMS);
  }
  function safeToActivate(el) {
    if (!el || !visible(el)) return {ok:false,code:"STALE_OR_HIDDEN"};
    const text=fold(label(el));
    if (/(?:payer|paiement|acheter|achat|commander|supprimer|effacer|delete|remove|purchase|pay|envoyer|submit|confirmer la commande|log out|deconnexion)/.test(text)) {
      return {ok:false,code:"SENSITIVE_ACTION"};
    }
    if (el.closest("form") && (el.tagName==="BUTTON" && (el.getAttribute("type")||"submit").toLowerCase()==="submit" ||
        el.tagName==="INPUT" && el.type==="submit")) {
      return {ok:false,code:"FORM_SUBMISSION_BLOCKED"};
    }
    if (el.tagName==="A") {
      if (el.hasAttribute("download")) return {ok:false,code:"DOWNLOAD_BLOCKED"};
      try {
        const dest=new URL(el.getAttribute("href"),location.href);
        if (!["http:","https:"].includes(dest.protocol)) return {ok:false,code:"UNSAFE_LINK"};
        if (dest.origin!==location.origin) return {ok:false,code:"CROSS_SITE_REQUIRES_CONFIRMATION"};
      } catch (_) { return {ok:false,code:"UNSAFE_LINK"}; }
    }
    return {ok:true};
  }
  function resolve(target) {
    const all=interactive();
    if (!target) return {ok:false,code:"NO_TARGET"};
    if (target.kind==="index") {
      if (listedUrl!==location.href || Date.now()-listedAt>60000) return {ok:false,code:"LIST_EXPIRED"};
      const el=snapshot[target.index-1];
      if (!el || !all.includes(el)) return {ok:false,code:"LIST_CHANGED"};
      return {ok:true,el};
    }
    if (target.kind!=="label" || typeof target.label!=="string") return {ok:false,code:"INVALID_TARGET"};
    const name=fold(target.label);
    const matched=all.filter(el=>(target.role==="any" || role(el)===target.role) &&
      (fold(label(el))===name || fold(label(el)).startsWith(name+" ")));
    if (!matched.length) return {ok:false,code:"ELEMENT_NOT_FOUND"};
    if (matched.length!==1) return {ok:false,code:"AMBIGUOUS_ELEMENT",count:matched.length};
    return {ok:true,el:matched[0]};
  }
  function listElements() {
    snapshot=interactive();
    listedAt=Date.now();
    listedUrl=location.href;
    return {ok:true,items:snapshot.map((el,i)=>({index:i+1,role:role(el),label:label(el)}))};
  }
  function click(el,startUrl) {
    if (location.href!==startUrl) return {ok:false,code:"PAGE_CHANGED"};
    const allowed=safeToActivate(el);
    if (!allowed.ok) return allowed;
    // Do not send synthetic keyboard events or execute page-supplied scripts.
    el.click();
    return {ok:true,action:"clicked",label:label(el)};
  }
  function waitFor(target,timeoutMs) {
    const startUrl=location.href;
    const ms=Math.min(MAX_WAIT,Math.max(250,Number(timeoutMs)||8000));
    if (waiting) waiting.abort("CANCELLED");
    return new Promise(resolvePromise=>{
      let done=false,observer,timer,stability;
      function finish(outcome) {
        if(done)return;
        done=true;observer?.disconnect();clearTimeout(timer);clearTimeout(stability);
        if(waiting===handle)waiting=null;
        resolvePromise(outcome);
      }
      const handle={abort:(reason)=>finish({ok:false,code:reason})};
      waiting=handle;
      function check() {
        if(done)return;
        if(location.href!==startUrl)return finish({ok:false,code:"PAGE_CHANGED"});
        if(document.visibilityState==="hidden")return finish({ok:false,code:"TAB_HIDDEN"});
        const found=resolve(target);
        if (!found.ok || !safeToActivate(found.el).ok) return;
        const candidate=found.el,rect=candidate.getBoundingClientRect();
        clearTimeout(stability);
        stability=setTimeout(()=>{
          if(done)return;
          const now=resolve(target);
          if (now.ok && now.el===candidate && safeToActivate(now.el).ok &&
              Math.abs(rect.x-now.el.getBoundingClientRect().x)<2 &&
              Math.abs(rect.y-now.el.getBoundingClientRect().y)<2) finish({ok:true,el:candidate,url:startUrl});
        },120);
      }
      observer=new MutationObserver(check);
      observer.observe(document.documentElement,{childList:true,subtree:true,attributes:true,
        attributeFilter:["hidden","disabled","aria-disabled","aria-hidden","class","style","href"]});
      timer=setTimeout(()=>finish({ok:false,code:"ELEMENT_TIMEOUT"}),ms);
      check();
    });
  }
  async function execute(action) {
    if (!action || typeof action!=="object") return {ok:false,code:"BAD_PLAN"};
    if (action.type==="cancel") {
      if(waiting)waiting.abort("CANCELLED");
      return {ok:true,action:"cancel"};
    }
    if (!["list","scroll","wait","click","wait_click"].includes(action.type))
      return {ok:false,code:"UNKNOWN_ACTION"};
    if (action.type==="list")return listElements();
    if (action.type==="scroll") {
      if(!["up","down"].includes(action.direction))return {ok:false,code:"BAD_DIRECTION"};
      window.scrollBy({top:(action.direction==="down"?1:-1)*Math.max(200,innerHeight*0.65),behavior:"instant"});
      return {ok:true,action:"scrolled"};
    }
    if (action.type==="click") {
      const target=resolve(action.target);
      return target.ok?click(target.el,location.href):target;
    }
    if (action.type==="wait" || action.type==="wait_click") {
      if(!action.target || action.target.kind!=="label" || !["any","button","link"].includes(action.target.role) ||
         typeof action.target.label!=="string" || action.target.label.length>100 ||
         !Number.isInteger(action.timeout_ms) || action.timeout_ms>MAX_WAIT)
        return {ok:false,code:"BAD_WAIT"};
      const found=await waitFor(action.target,action.timeout_ms);
      if(!found.ok)return found;
      return action.type==="wait_click"?click(found.el,found.url):{ok:true,action:"ready",label:label(found.el)};
    }
    return {ok:false,code:"BAD_ACTION"};
  }
  browser.runtime.onMessage.addListener(message=>{
    if(!message || message.type!=="myna.nav.execute" || message.version!==1)return false;
    return Promise.resolve(execute(message.action)).then(result=>{
      // Never return page HTML, form values, passwords, or arbitrary DOM text.
      return result;
    });
  });
  addEventListener("pagehide",()=>{if(waiting)waiting.abort("PAGE_CLOSED");snapshot=[];});
})();

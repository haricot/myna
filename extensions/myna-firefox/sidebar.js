"use strict";
// The Myna ASR already dictates into the sidebar textarea through the normal
// Firefox text input backend. No new microphone, LLM, or native host is used.
const $=id=>document.getElementById(id);
let activePattern=null;
function patternFor(urlText) {
  try {
    const u=new URL(urlText);
    if (!["http:","https:"].includes(u.protocol)) return null;
    return u.protocol+"//"+u.hostname+"/*";
  } catch (_) {return null;}
}
const status = s => { $("status").textContent=s; };
async function currentTab() {
  const tabs=await browser.tabs.query({active:true,currentWindow:true});
  const tab=tabs[0];
  if(!Number.isInteger(tab?.id) || !patternFor(tab.url))return null;
  return tab;
}
async function refresh() {
  const tab=await currentTab();
  activePattern=tab?patternFor(tab.url):null;
  $("site").textContent=tab?"Onglet : "+new URL(tab.url).origin:"Aucun onglet web autorisable.";
  const granted=activePattern && await browser.permissions.contains({origins:[activePattern]});
  $("grant").disabled=!activePattern || granted;
  if(!activePattern)status("Clic droit dans la page → Myna Navigation — autoriser ce site. Seules les pages HTTP(S) sont prises en charge.");
  else if(!granted)status("Clic droit dans la page → Myna Navigation — autoriser ce site, puis réessayez.");
  return Boolean(granted);
}
$("grant").addEventListener("click",async()=>{
  if(!activePattern)return;
  // A direct user gesture is required for optional host permission.
  try {
    const ok=await browser.permissions.request({origins:[activePattern]});
    status(ok?"Site autorisé.":"Permission refusée.");
    await refresh();
  } catch(err) {status("Autorisation impossible : "+err.message);}
});
async function perform(action) {
  const tab=await currentTab();
  if(!tab || !await browser.permissions.contains({origins:[patternFor(tab.url)]})) {
    status("Site non autorisé. Cliquez d'abord sur « Autoriser ce site ».");
    return;
  }
  try {
    await browser.scripting.executeScript({target:{tabId:tab.id},files:["navigation.js"]});
    const response=await browser.tabs.sendMessage(tab.id,{type:"myna.nav.execute",version:1,action});
    if(!response?.ok) {
      status("Action refusée / indisponible : "+(response?.code||"AUCUNE_REPONSE"));
      return;
    }
    status("Résultat : "+(response.action||"éléments trouvés"));
    if(Array.isArray(response.items)) {
      $("results").replaceChildren();
      for(const item of response.items) {
        const li=document.createElement("li");
        li.textContent=item.index+" · "+item.role+" · "+item.label;
        $("results").append(li);
      }
    } else $("results").replaceChildren();
  } catch(err) {status("Erreur Firefox : "+err.message);}
}
$("form").addEventListener("submit",event=>{
  event.preventDefault();
  const answer=globalThis.MynaIntentRouter.route($("phrase").value);
  if(answer.status!=="ready"){status("Commande non reconnue : aucune action exécutée.");return;}
  if(!globalThis.MynaIntentRouter.validate(answer.action)){
    status("Plan invalide : aucune action exécutée.");return;
  }
  void perform(answer.action);
});
$("list").addEventListener("click",()=>void perform({type:"list"}));
$("cancel").addEventListener("click",()=>void perform({type:"cancel"}));
browser.permissions.onAdded.addListener(()=>void refresh());
browser.tabs.onActivated.addListener(()=>void refresh());
browser.tabs.onUpdated.addListener((_id,change)=>{if(change.url)void refresh();});
void refresh();

const test=require("node:test");
const assert=require("node:assert/strict");
const Router=require("../intent-router.js");
const cases=[
  ["affiche les liens","list"],["Montre les boutons","list"],
  ["descends dans la page","scroll"],["remonte dans la page","scroll"],
  ["clique sur Rechercher","click"],["ouvre le troisième lien","click"],
  ["ouvre le lien numéro 3","click"],
  ["attends que le bouton Continuer apparaisse","wait"],
  ["Dès que le bouton Continuer apparaît, clique dessus","wait_click"],
];
for(const [input,expected] of cases)test(input,()=>{
  const decision=Router.route(input);
  assert.equal(decision.status,"ready");
  assert.equal(decision.action.type,expected);
  assert.ok(Router.validate(decision.action));
});
for(const input of ["supprime ce dossier","execute rm -rf","ouvre terminal","",
  "p".repeat(500)])test("refuse "+input.slice(0,30),()=>{
  assert.equal(Router.route(input).status,"unmatched");
});
test("refuses unknown operations or fake LLM actions",()=>{
  assert.equal(Router.validate({type:"eval",code:"alert(1)"}),false);
  assert.equal(Router.validate({type:"click",target:{kind:"index",index:61}}),false);
  assert.equal(Router.validate({type:"click",target:{kind:"label",role:"any",label:"<script>"}}),false);
});

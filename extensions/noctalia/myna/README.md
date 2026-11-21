# Myna pour Noctalia 5.6.0

Plugin local **haricot/myna** : indicateur d'état + panneau de contrôle.
Il conserve le provider Candle/Nemotron, SmartTurn, les ancres Firefox/egui
et le raccourci Niri `Ctrl+Alt+M` ; aucune recompilation Rust n'est nécessaire.

## Commandes du menu

| Fonction | Commande réellement utilisée |
| --- | --- |
| Démarrer le client + provider | `systemctl --user start myna-desktop.service myna-candle-provider.service` |
| Arrêter les deux services | `systemctl --user stop myna-desktop.service myna-candle-provider.service` |
| Lire les états | `systemctl --user is-active ...` |
| Reconnaissance : Streaming / Offline / Hybride | `~/.local/bin/myna-control recognition`, puis `recognition-apply streaming\|offline\|hybrid` |
| Capture : SmartTurn automatique / Manuel continu | `~/.local/bin/myna-control mode`, puis `mode-apply smart\|manual` |

Les boutons sélectionnés correspondent aux valeurs réelles retournées par le
script local, non à des valeurs fictives. Un choix est appliqué immédiatement,
un seul à la fois. Il peut redémarrer des services et interrompre une dictée
en cours ; le script `myna-control` en garde la responsabilité.

**Streaming** décrit le moteur de reconnaissance en temps réel ;
**Manuel continu** désactive uniquement l'arrêt automatique côté client
(`--auto-endpoint`) : la dictée continue jusqu'au deuxième appui sur
`Ctrl+Alt+M`. **Hybride** conserve le provisoire streaming et le final offline.
Ces deux familles de réglages sont indépendantes.

Le plugin utilise uniquement `noctalia.runAsync` avec une liste d'arguments
fixes validés : pas de `sh -c`, sudo, texte ASR dans des commandes, shell libre,
ni nouvelle permission système. Les services restent pilotés par systemd user.

## Installer / actualiser

```bash
cd ~/tmp/myna-candle-client-native
git pull --ff-only

# Si la source path haricot-myna a déjà été ajoutée, le panneau est
# normalement rechargé lorsque son fichier .luau change :
noctalia msg panel-toggle haricot/myna:panel
```

Si la source n'a jamais été ajoutée :

```bash
noctalia msg plugins source add haricot-myna path "$PWD/extensions/noctalia"
noctalia msg plugins enable haricot/myna
noctalia msg panel-toggle haricot/myna:panel
```

Le widget de barre peut être placé dans Noctalia Settings → Bar → Add Widget →
Myna (`haricot/myna:status`). Un clic sur l'icône ouvre le panneau.

Si l'ancienne taille de panneau persiste, désactivez puis réactivez
`haricot/myna` dans Settings → Plugins, sans redémarrer le provider CUDA.

## Vérifier sans modifier les réglages

```bash
~/.local/bin/myna-control recognition
~/.local/bin/myna-control mode
systemctl --user is-active myna-desktop.service myna-candle-provider.service
```

Les commandes `mode-apply` et `recognition-apply` sont celles de l'installation
Myna native personnalisée ; elles ne sont pas garanties par le Myna officiel.

Statut : modification de plugin inspectée statiquement et publiée sur GitHub.
Les clics et le rendu sur une session Niri/Noctalia réelle restent à qualifier.

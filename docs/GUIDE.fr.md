# Guide de démarrage (FR)

Objectif : faire tourner un serveur Trackmania 2020 avec PitWall, vérifier que tout marche, puis
préparer la mini-cup à 8.

## 1. Créer le compte du serveur dédié

1. Va sur <https://players.trackmania.com/server/dedicated> et connecte-toi avec ton compte
   Ubisoft.
2. Crée un serveur dédié : tu obtiens un **login** et un **mot de passe** propres au serveur. Ce
   ne sont pas ceux de ton compte joueur.
3. Garde-les pour l'étape suivante. Ne les écris jamais dans un fichier versionné.

## 2. Lancer le serveur dédié (Docker)

Sur la machine (WSL avec Docker Desktop, ou la Debian) :

```sh
cp .env.example .env
# Édite .env :
#   TM_MASTERSERVER_LOGIN / TM_MASTERSERVER_PASSWORD : le compte de l'étape 1
#   TM_AUTHORIZATION_SUPERADMIN_PASSWORD / _ADMIN_ / _USER_ : des mots de passe longs et aléatoires
docker compose up -d trackmania
docker compose logs -f trackmania     # attendre que le serveur soit prêt
```

Ports : le jeu utilise `2350` en TCP et UDP. Pour des joueurs hors de ton réseau, redirige-le sur
ta box vers la machine. Si l'adresse annoncée est fausse (NAT Docker), renseigne
`TM_SYSTEM_FORCE_IP_ADDRESS=ip_publique:2350`. Le port XML-RPC (5000) n'est publié que sur
`127.0.0.1`, il ne faut pas l'ouvrir.

> Même en LAN, Trackmania 2020 a besoin d'Internet : les joueurs se connectent avec leur compte
> Ubisoft, et le serveur s'enregistre auprès de Nadeo.

## 3. Lancer PitWall

```sh
cp pitwall.example.toml pitwall.toml
cargo run --release
```

PitWall lit `.env`. `PITWALL_SERVER__PASSWORD` y vaut déjà le mot de passe SuperAdmin. Tu dois voir :

```
INFO pitwall_core::runtime: connected to the game server name=Trackmania ...
```

Dans la console, tape `help` : la liste des commandes s'affiche.

### Devenir SuperAdmin en jeu

1. Rejoins le serveur depuis le jeu (Jouer → En ligne → Serveurs, cherche le nom défini dans
   `TM_SERVER_NAME`).
2. Dans la console PitWall, tape `players` : ton login s'affiche à côté de ton pseudo.
3. Mets-le dans `pitwall.toml`, section `[roles]`, clé `superadmins = ["ton_login"]`, puis relance.
   Sinon, en direct depuis la console : `role ton_login superadmin`. Ce changement est perdu au
   redémarrage.

## 4. Vérifications (checklist)

| Test | Comment | Attendu |
| --- | --- | --- |
| Connexion | lancer PitWall | `connected to the game server` dans les logs |
| Commandes | `/help` dans le chat | liste en français si ton jeu est en français |
| Widgets | rejoindre le serveur | classement live à droite, menu en bas à gauche |
| Changement de mode | `/mode cup` puis `/mode ta` | rechargement de la map, personne n'est déconnecté, message « Mode : … » |
| Bots | `/bot` (plusieurs fois), `/bot off` | des faux joueurs se connectent puis partent |
| Classement | finir une course en TA | ta ligne apparaît en glissant, temps affiché |
| Masquer | cliquer « ● Classement » dans le menu (en haut à gauche) | le widget disparaît instantanément, le choix est conservé |
| Panneau admin | être Referee ou plus | boutons Pause, Fin de manche… |
| Maps TMX | `/map tmx 361306 next` puis `/next` | map téléchargée et jouée |
| Dashboard | `PITWALL_API__ADMIN_TOKEN=xxx` dans `.env`, ouvrir <http://127.0.0.1:8080/admin> | console web, joueurs, journal d'audit |

## 5. Mini-cup à 8

`config/cups/mini-cup.toml` : 10 min de TimeAttack, puis une finale Cup pour les 4 premiers.

```
/cup load mini-cup
/cup open            → les joueurs tapent /join (ou /cup add <login>)
/cup start           → appel : chacun clique PRÊT (ou /ready), 90 s max
                       les qualifications se lancent toutes seules quand tout le monde est prêt
                       à la fin : résultats dans le chat, les 4 premiers passent en finale
                       finale : démarrage automatique après 20 s
/cup status full     → état et participants
/cup pause|resume    → pause technique ;  /cup advance → terminer la phase maintenant
/cup abort           → annuler
```

Si le serveur ou PitWall redémarre pendant la cup, l'état est relu depuis
`data/cups/active.json` et la cup reprend où elle en était.

Page publique de la cup : <http://127.0.0.1:8080/cup>. Historique et exports CSV/JSON :
<http://127.0.0.1:8080/history>.

## 6. Sans serveur Trackmania

```sh
cargo run -p pitwall-gbx --features mock --example mock_server        # faux serveur sur :5000
PITWALL_SERVER__PASSWORD=dev cargo run                                 # puis : mode cup
```

## 7. Déploiement automatique (PC → GitHub → Debian)

```
git push main ──► CI GitHub : fmt, clippy, tests
                  └─► image ghcr.io/maksiime/pitwall:sha-<commit>
                      └─► branche deploy : stack.yml épinglé sur cette image
                                     └─► Portainer (polling 5 min) ──► redéploie pitwall
```

- **Sur ton PC**, le hook `.githooks/pre-push` lance les mêmes vérifications que la CI avant
  chaque push. Il s'active une fois par clone : `git config core.hooksPath .githooks`. Pour le
  contourner : `git push --no-verify`.
- **Un push sur `main` redéploie PitWall**, pas le serveur Trackmania : seule l'image de PitWall
  change, et le conteneur `trackmania` n'est pas touché. PitWall redémarre en quelques secondes,
  les joueurs restent connectés. Pour un commit qui ne doit pas partir sur le serveur, ajoute
  `[skip deploy]` dans son message.
- **Image du serveur Trackmania** : la version est épinglée dans `deploy/portainer/stack.yml`.
  Chaque jour, le workflow « Trackmania server image » ouvre une PR quand Evo publie une version
  plus récente. La fusionner redéploie le serveur, qui redémarre et déconnecte les joueurs : à
  faire hors d'une cup.

### Mise en place (une fois)

1. **GitHub** → Settings → Actions → General → Workflow permissions : coche « Allow GitHub
   Actions to create and approve pull requests » (pour les PR de mise à jour de l'image).
2. Pousse sur `main` et attends que la CI soit verte : la branche `deploy` et le paquet
   `ghcr.io/maksiime/pitwall` apparaissent. Le paquet est public comme le dépôt, Portainer n'a
   pas besoin d'identifiants.
3. **Portainer** : note le nom de l'ancienne stack, puis supprime-la **sans** supprimer ses
   volumes (Volumes doit toujours lister `<nom>_tm-userdata` : maps et config du serveur).
4. Stacks → Add stack → **le même nom** (le volume est alors réutilisé) → **Repository** :
   - Repository URL : `https://github.com/MaKSiiMe/PitWall`
   - Repository reference : `refs/heads/deploy`
   - Compose path : `stack.yml`
   - GitOps updates : activé, mécanisme **Polling**, intervalle `5m`. Laisse « Re-pull image »
     et « Force redeployment » désactivés : on ne redéploie que si `stack.yml` change.
   - Environment variables → Advanced mode : colle `deploy/portainer/stack.env.example` et
     remplis-le (mêmes valeurs que l'ancienne stack, plus `PITWALL_SUPERADMINS`).
5. Deploy. Vérification : dans Portainer, les logs du conteneur `pitwall` affichent
   `connected to the game server`. Le dashboard est sur `http://100.124.153.20:8080`.
   « Attach » sur le conteneur `pitwall` donne la console (tape `help`).
6. Sur ton PC, ne lance plus `cargo run` contre ce serveur : deux contrôleurs se marchent dessus.
   Pour développer, arrête d'abord le conteneur `pitwall` dans Portainer.

L'historique local (`data/pitwall.db`) ne suit pas. Pour le garder, après le premier
déploiement : arrête `pitwall`, copie le fichier dans le volume `<nom>_pitwall-data`, puis
redémarre.

## 8. Changer les maps

**En jeu, sans quitter le serveur** : tape `/maps` ou clique sur **Maps** dans le panneau admin.
La fenêtre montre la playlist page par page, avec la map en cours surlignée :

| Bouton | Effet |
| --- | --- |
| ▲ ▼ | monter / descendre la map d'une place |
| » | la jouer juste après la map en cours |
| > | la jouer maintenant |
| X | la retirer de la playlist |
| Vider la playlist | garde seulement la map en cours (deux clics pour confirmer) |
| Ajouter | colle un id ou une URL Trackmania Exchange, une URL de campagne trackmania.io (`https://trackmania.io/#/campaigns/<club>/<campagne>`) ou un uid de map, puis Entrée, « Ajouter » ou « Jouer ensuite » |
| Club Nadeo | id du club + id de la campagne ou du salon, puis « Campagne » ou « Salon » : toutes ses maps sont ajoutées |
| Recherche TMX | tape un nom, Entrée : les résultats s'affichent avec **+** (ajouter) et **»** (jouer ensuite) |

La fenêtre se met à jour toute seule après chaque changement, y compris ceux d'un autre admin, et
à chaque nouvelle map. X en haut à droite la ferme.

Les mêmes actions existent en commandes (chat ou console PitWall). Les numéros sont ceux de
`/map list` :

| Commande | Effet |
| --- | --- |
| `/map list` | playlist actuelle (» = map en cours) |
| `/map move <de> <vers>` | déplacer une map, ex. `/map move 12 1` |
| `/map next <n>` / `/map play <n>` | jouer la map n ensuite / maintenant |
| `/map remove [n]` | retirer la map n (sans argument : la map en cours) |
| `/map clear` | ne garder que la map en cours |
| `/map add <lien ou id> [next]` | reconnaît un id/URL TMX, une URL de campagne trackmania.io ou un uid |
| `/map tmx <id ou URL> [next]`, `/map search <texte>` | Trackmania Exchange |
| `/map uid <uid> [next]` | une map Nadeo par son uid |
| `/map club <club id> <campaign id>` ou `/map club <URL trackmania.io>` | toutes les maps d'une campagne de club, de n'importe quel club public |
| `/map room <club id> <room id>` | toutes les maps d'un salon de club |
| `/map local [fichier]` | fichiers `.Map.Gbx` du dossier de PitWall (`/data/maps` dans le conteneur) |

Les maps Nadeo demandent le compte du serveur dédié (`TM_MASTERSERVER_LOGIN`, déjà dans la
stack). Les ids de club, de campagne et de salon se lisent sur trackmania.io. Un salon réservé aux
membres de son club est refusé (le compte serveur n'en est pas membre), et un salon relié à un
serveur dédié ne liste pas ses maps.

Les maps téléchargées vont dans `UserData/Maps/PitWall/` sur le serveur. Dans la stack,
PitWall les écrit directement dans le volume du serveur (`PITWALL_SERVER__MAPS_PATH`), quelle que
soit leur taille. Sans ce volume partagé (PitWall lancé sur ton PC), elles passent par
`WriteFile`, limité à environ 3 Mo par map : les plus grosses sont refusées avec un message. Pour une cup, chaque
phase peut avoir sa propre liste (`maps = ["PitWall/tmx-123.Map.Gbx", …]` dans le `.toml` de
la cup), chargée au début de la phase.

**Sauvegarde** : après chaque changement de playlist et à chaque début de map, PitWall
enregistre la playlist et le mode dans `MatchSettings/default.txt`, le fichier avec lequel le
serveur démarre. Un redémarrage du serveur reprend donc la même playlist, dans le même mode.
La stack Portainer règle les deux côtés avec la même variable `TM_GAME_SETTINGS`. Hors de la
stack, garde `PITWALL_SERVER__PLAYLIST_FILE` (préfixée de `MatchSettings/`) égale à
`TM_GAME_SETTINGS`.

## À vérifier sur un vrai serveur

Ces points suivent la documentation et le code d'autres contrôleurs, mais n'ont pas pu être
testés sans serveur réel :

1. **ManiaScript des widgets** : syntaxe et API TM2020 (partage `declare … for LocalUser` entre
   deux pages, `fromjson`, `AnimMgr`). En cas d'erreur, les logs du jeu (Openplanet ou la console
   ManiaLink) indiquent la ligne fautive.
2. **`LoadMatchSettings`** applique-t-il les réglages `S_*` du nouveau script dès le premier
   chargement ? Sinon, PitWall les réapplique automatiquement au chargement de la map, et le log
   affiche `re-applying mode settings`.
3. **`WriteFile`** crée-t-il les sous-dossiers `MatchSettings/PitWall/` et `PitWall/` ?
4. **Commandes de chat** : un message `/xxx` est-il visible des autres joueurs ? Si oui, il faudra
   activer `ChatEnableManualRouting`.
5. ~~Version d'API~~ : vérifié, le serveur annonce `2023-03-25` (valeur par défaut de PitWall).
6. **API Nadeo** avec le compte serveur dédié : `/map uid` et `/map club`.
7. **Gestionnaire de maps** : champs de saisie (`entry`, Entrée et clic lus par ManiaScript),
   ▲▼ sur la map en cours (elle est retirée puis remise dans la playlist : elle doit continuer
   sans coupure, et la suivante doit être la bonne).
8. **Sauvegarde de la playlist** : `SaveMatchSettings` écrit-il aussi le mode et ses réglages
   `S_*` ? Test : réordonner, passer en `/mode cup`, redémarrer le conteneur `trackmania`, puis
   vérifier l'ordre et le mode.
9. **Tableau Tab de PitWall** (calque `ScoresTable`) : il remplace celui de Nadeo, masqué par
   `Common.UIModules.SetProperties` (`Race_ScoresTable`). Pour garder celui de Nadeo :
   `[plugins.ui] native_scoreboard = true`.

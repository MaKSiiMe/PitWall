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
| Écart aux CP | refaire un run | écart vert ou rouge au centre à chaque checkpoint |
| Masquer | cliquer « ● Classement » dans le menu | le widget disparaît, le choix est conservé |
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

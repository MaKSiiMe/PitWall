# Enregistrer une session et en faire un test

Quand un bug n'arrive que sur le vrai serveur, PitWall peut enregistrer tout ce qu'il échange
avec lui (le trafic GbxRemote brut). On rejoue ensuite cet enregistrement dans le faux serveur
des tests : le bug devient un test qui échoue, on le corrige, et il ne revient plus.

## 1. Enregistrer sur le serveur

Désactivé par défaut. Dans Portainer, variables de la stack :

```
PITWALL_RECORDING=true
```

puis « Update the stack » (seul le conteneur `pitwall` redémarre). Hors stack, dans
`pitwall.toml` :

```toml
[recording]
enabled = true
dir = ""          # "" = <data_dir>/recordings, soit /data/recordings dans le conteneur
max_file_mb = 20  # un nouveau fichier commence au-delà de cette taille
max_files = 10    # seuls les plus récents sont gardés
```

Au démarrage, le log affiche `GbxRemote traffic recording enabled`. Joue la session jusqu'au
bug, note l'heure, puis remets `PITWALL_RECORDING=false` : les fichiers grossissent vite
(chaque checkpoint de chaque joueur y est).

### Ce que contient un fichier

Des fichiers `gbx-<horodatage>.jsonl`, une ligne par message :

```json
{"t":1729771234567,"dir":"out","handle":2147483649,"xml":"<methodCall>…"}
{"t":1729771234570,"dir":"in","handle":2147483649,"xml":"<methodResponse>…"}
{"t":1729771234600,"event":"connected"}
```

- `t` : heure en millisecondes (horodatage Unix) ;
- `dir` : `out` = envoyé par PitWall (appels), `in` = envoyé par le serveur (réponses et
  callbacks) ;
- `handle` : numéro du message, le même pour un appel et sa réponse ;
- `event` : connexion et déconnexion au serveur.

**Les mots de passe sont masqués** (`***`) avant d'être écrits : `Authenticate`,
`SetServerPassword` et les autres méthodes de mot de passe, les réponses de
`GetServerPassword`…, tout champ dont le nom contient `password`, et un `/password …` tapé dans
le chat. Un message illisible est enregistré sans son contenu.

En revanche, **les logins, pseudos et messages du chat des joueurs sont en clair** : ces
fichiers restent sur le serveur, ne les partage pas tels quels.

### Récupérer les fichiers

Sur la Debian :

```sh
docker cp pitwall:/data/recordings ./recordings
```

Le fichier qui t'intéresse est celui dont l'horodatage précède l'heure du bug (l'horodatage est
dans le nom ; `date -d @1729771234` le convertit, sans les trois derniers chiffres).

## 2. Rejouer à la main

Pour voir ce qui se passe avec un vrai PitWall (logs, console), sur ton PC :

```sh
cargo run -p pitwall-gbx --features mock --example replay -- recordings/gbx-….jsonl 10
PITWALL_SERVER__PASSWORD=dev cargo run          # dans un second terminal
```

Le faux serveur écoute sur `127.0.0.1:5000` et répond avec les réponses enregistrées. Il rejoue
les callbacks dès que PitWall est connecté. Le dernier argument divise les délais : `1` pour le
temps réel, `10` pour aller dix fois plus vite.

## 3. En faire un test

1. **Anonymiser** : les logins et identifiants de compte deviennent des faux, les pseudos
   `Player 1`, `Player 2`…, et le chat qui n'est pas une commande devient `…`.

   ```sh
   cargo run -p pitwall-gbx --features mock --example anonymize -- \
       recordings/gbx-….jsonl crates/core/tests/traces/mon-bug.jsonl
   ```

   **Relis le fichier avant de le committer** : le dépôt est public, et un texte libre (nom de
   map, raison d'un kick…) peut rester. Garde seulement les lignes utiles au bug quand c'est
   possible : un fichier court se relit mieux.

2. **Écrire le test** dans [crates/core/tests/replay.rs](../crates/core/tests/replay.rs) :
   `replay(&trace)` rejoue la trace contre un contrôleur neuf et renvoie les événements qu'il a
   publiés.

   ```rust
   #[tokio::test]
   async fn mon_bug_ne_revient_pas() {
       let trace = Trace::load(&Path::new(env!("CARGO_MANIFEST_DIR"))
           .join("tests/traces/mon-bug.jsonl"))
       .unwrap();
       let events = replay(&trace).await;
       assert!(/* ce qui aurait dû se passer */, "{events:?}");
   }
   ```

   Pour un bug d'un plugin (UI, cup…), même principe dans les tests du plugin : ajoute le plugin
   au contrôleur avant `run`.

3. Lance `cargo test -p pitwall-core --test replay` : le test doit **échouer** avant la
   correction et passer après.

[crates/core/tests/traces/ta-session.jsonl](../crates/core/tests/traces/ta-session.jsonl) est
un exemple : une courte session de contre-la-montre (un joueur arrive, parle, finit), enregistrée
puis anonymisée avec ces outils. Le test `the_sample_trace_is_anonymized` vérifie que les traces
du dépôt passent bien par `anonymize`.

## Limites

- Les réponses sont rendues par méthode, dans l'ordre d'enregistrement : le n-ième
  `GetPlayerList` reçoit la n-ième réponse enregistrée, puis la dernière en boucle. Si la
  correction change l'ordre ou le nombre des appels de PitWall, la trace reste utilisable, mais
  les réponses peuvent ne plus tomber au même moment.
- Une déconnexion enregistrée n'est pas rejouée (le faux serveur reste connecté). Pour un bug de
  reconnexion, il faut l'écrire à la main, comme `reconnects_after_the_server_comes_back` dans
  [crates/core/tests/runtime.rs](../crates/core/tests/runtime.rs).
- Si le disque ne suit pas, des messages sont abandonnés plutôt que de ralentir PitWall. Le log
  affiche alors `recording falls behind, frames dropped`.

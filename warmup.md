# Warmup — Défilement humain des feeds vidéo (Instagram & TikTok)

> Fichier d'instructions pour les agents d'exécution Switch. Référencé depuis `agent.md`.
> But : rendre le compte local « actif » en scrolant les feeds vidéo (Instagram Reels et TikTok « Pour toi ») depuis le téléphone branché en USB, **de la façon la plus humaine possible**, pendant **au plus 30 minutes**, en **alternant automatiquement entre les deux applis**.

---

## 1. Objectif

Faire défiler les feeds vidéo sur le téléphone (Instagram Reels **et** TikTok, alternés toutes les quelques minutes) exactement comme un humain qui regarde des vidéos : gestes irréguliers, pauses naturelles, jamais deux gestes identiques, durée totale plafonnée à 30 min. La navigation d'app (ouverture de chacune, bascule entre les deux) est faite automatiquement par ADB, sans toucher au téléphone.

C'est un « warmup » : on rend la session/le compte actif sans aucune requête API ni scraping.

## 2. Règles obligatoires (non négociables)

1. **Tout est aléatoire.** Aucun geste ne doit être identique au précédent.
2. **IMPÉRATIF — on scroll AVANT la fin de la vidéo** : une vidéo ne doit **jamais** se terminer sans qu'on ait scrollé. Si la progression de la lecture est connue, le geste part à **55–92 %** de la vidéo (toujours avant la fin) ; sinon, temps de visionnage **court et aléatoire (2,5–7 s)**, en dessous de la durée des clips typiques.
3. Le temps de visionnage n'est **jamais le même deux fois** : le pourcentage (ou le plafond) est relancé au hasard sur chaque vidéo.
4. **Le geste « vers le bas »** (défiler le feed vers le bas → passer à la vidéo suivante ; le doigt balaye vers le **haut** de l'écran) :
   - ne doit **jamais** être le même : coordonnées, angle, durée et vitesse tirés au hasard à chaque fois ;
   - ne doit **jamais** être parfaitement droit : toujours légèrement **de travers** (diagonal, avec une dérive horizontale) ;
   - doit être **UN seul geste continu** (un seul `input swipe`) : jamais de segmentation, chaque segment serait un défilement à part et ferait passer plusieurs vidéos d'un coup ;
   - ne doit jamais passer **plus d'une vidéo** par geste (course ~22–34 % de l'écran, vitesse modérée).
5. **Variations humaines occasionnelles** (le script les gère) :
   - ~4 % : petit geste de retour en arrière (vidéo précédente) ;
   - ~6 % : double geste rapide (skip) — deux vidéos enchaînées ;
   - **Carousels (Instagram & TikTok)** : quand un post est un carousel, il faut slide VERS LA GAUCHE pour voir les vignettes suivantes — le script les feuillette parfois (1–3 slides, aléatoire) avant de passer à la vidéo suivante, sur Instagram comme sur TikTok. Fréquence réglable par `--carousel-prob` (défaut 0,4).
   - **Détour profil (+ stories parfois)** : parfois ouvre un profil de créateur (clic sur son avatar), le parcourt 1–2 vignettes, **et regarde parfois ses STORIES** (le viewer s'ouvre directement depuis l'avatar si le créateur a une story active, ou via un story de la rangée du profil ; 1–4 stories regardées puis fermées) avant de REVENIR sur les reels (bouton retour). Jamais deux fois d'affilée. Fréquence : `--profile-prob` (défaut 0,15) et `--story-prob` (défaut 0,5).
   - **Like parfois** : une vidéo en cours de lecture est parfois likée (60 % double-tap sur la vidéo, sinon tap sur le bouton cœur) — jamais deux vidéos d'affilée. Fréquence réglable par `--like-prob` (défaut 0,20).
   - **Commentaires parfois** : ouvre les commentaires d'une vidéo, les fait défiler lentement (2–5 swipes) en **s'arrêtant parfois sur un commentaire** pour le lire (pause 2,5–5,5 s, max 2 arrêts), parfois en le likant, **puis scroll TOUJOURS de suite** (un arrêt n'arrive jamais sur le dernier swipe), puis revient au feed. Jamais deux fois d'affilée. Fréquence réglable par `--comments-prob` (défaut 0,15).
6. **Alternance automatique entre les applis** toutes les `--cycle-min` minutes (~3 min par défaut) : bascule Instagram Reels ⇄ TikTok « Pour toi » gérée par ADB.
7. **Durée totale : jusqu'à 30 minutes maximum**, puis arrêt propre. Ne jamais dépasser.
8. **Ne jamais intervenir sur le téléphone** pendant le warmup (pas de toucher, pas d'appel ADB concurrent).

## 3. Exécution

L'outil implémenté est `warmup_scroll.py` (dossier `E:\instagram-likes-api`).

```bash
cd /e/instagram-likes-api
./.venv/Scripts/python warmup_scroll.py                    # 30 min, alterne IG + TikTok (~3 min chacun)
./.venv/Scripts/python warmup_scroll.py --max-min 20        # durée totale réduite
./.venv/Scripts/python warmup_scroll.py --cycle-min 4       # 4 min par app avant bascule
./.venv/Scripts/python warmup_scroll.py --apps tiktok       # une seule app
./.venv/Scripts/python warmup_scroll.py --carousel-prob 0.6  # feuillette plus les carousels
./.venv/Scripts/python warmup_scroll.py --profile-prob 0.25  # détours profil plus fréquents
./.venv/Scripts/python warmup_scroll.py --like-prob 0.35     # likes plus fréquents
./.venv/Scripts/python warmup_scroll.py --comments-prob 0.3   # commentaires plus fréquents
./.venv/Scripts/python warmup_scroll.py --log-gestures gestures/mes_scrolls.json  # journalise les gestes
./.venv/Scripts/python gesture_playback.py replay mes_scrolls --repeat 5   # rejoue les gestes
./.venv/Scripts/python warmup_scroll.py --gestures-file gestures/mon_style.json  # scroll avec TES gestes (21 swipes enregistrés)
./.venv/Scripts/python warmup_scroll.py --dry-run           # simulation (aucun geste envoyé)
```

Le script affiche chaque geste en direct : coordonnées, durée, nombre de segments, pause suivante, temps écoulé, et le nom de l'app active à chaque cycle.

## 4. Prérequis (à vérifier AVANT de lancer)

- Téléphone branché en USB, **débogage USB (ADB) actif**, écran **déverrouillé et allumé**.
- `adb devices` liste bien le téléphone (`...  device`).
- **Instagram et/ou TikTok installés et connectés** sur le téléphone (le script détecte les applis présentes et ignore celles absentes).
- Pas besoin d'ouvrir l'app à la main : le script lance chaque feed via deep-link/activité ADB. Sur Instagram il ouvre directement un reel réel (`https://www.instagram.com/reel/<code>/`, choisi au hasard) : on arrive **dans le lecteur Reels plein écran** et on swipe de vidéo en vidéo (pas sur la grille). Sur TikTok, il ouvre l'activité principale (flux « Pour toi »).
- Le téléphone reste **posé sans être touché** pendant toute la durée du warmup.

## 5. Déroulé type pour un agent

1. Vérifier la connexion ADB : `adb devices` (sinon : téléphone débranché, arrêter et signaler).
2. Vérifier que les applis IG/TikTok sont installées et connectées (sinon le script les ignorera).
3. Lancer `warmup_scroll.py` (commande ci-dessus, durée demandée max 30 min, `--cycle-min` pour la fréquence de bascule).
4. **Ne jamais intervenir sur le téléphone** pendant l'exécution (pas de toucher, pas d'appel ADB concurrent).
5. Surveiller les logs : l'app active doit changer tous les `--cycle-min`, les pauses doivent varier (3–15 s, parfois plus longues), les gestes ne doivent jamais se répéter.
6. À la fin (durée atteinte ou arrêt) : rapporter le résumé du script (gestes par app, durée réelle, pauses longues). En cas d'arrêt prématuré (« Téléphone débranché ») ou d'app non ouvrable (signalée `! impossible d'ouvrir …`), le signaler dans le rapport.

## 6. Notes techniques

- **Sur Instagram, le warmup se fait « dans les reels »** : le script ouvre un reel réel au hasard parmi une piscine de shortcodes vérifiés (`com.instagram.android` → activité `ClipsUrlHandlerActivity`, le lecteur Reels plein écran), et on y swipe de vidéo en vidéo. L'app est donc dans le lecteur de reels, pas sur la grille du profil.
- **Navigation d'app (ADB)** : Instagram via `adb shell am start -a android.intent.action.VIEW -d 'https://www.instagram.com/reel/<shortcode>/'` (lecteur Reels plein écran), TikTok via `adb shell am start -n com.zhiliaoapp.musically/com.ss.android.ugc.aweme.splash.SplashActivity` (flux « Pour toi »). Le script confirme que l'app est au premier plan (`mCurrentFocus`) avant de commencer à défiler, sinon il passe à l'app suivante.
- **« Avant la fin »** : le script lit la progression réelle via `adb shell dumpsys media_session` quand l'app la publie (scroll à 55–92 %, avec marge de sécurité avant la fin) ; sinon il applique un plafond de visionnage court et aléatoire (2,5–7 s) pour rester avant la fin des clips typiques. `warmup_scroll.py` indique « avant-fin après Xs » à chaque geste.
- **« Vers le bas »** = faire défiler le feed vers le bas (vidéo suivante) ; le doigt balaye vers le **haut** de l'écran (`adb shell input swipe y_bas → y_haut`).
- **Swipe d'une vidéo** : le swipe vertical est **un seul geste continu** parcourant **~22–34 % de la hauteur d'écran** (départ 68–78 %), durée 600–1000 ms — il passe **exactement UNE vidéo**, sans fling ni multi-passe.
- **Plancher 2,5 s** : aucun swipe de transition ne part avant **2,5 s** de visionnage (même le double-skip) — jamais de défilement trop rapide.
- **Enregistrement/rejeu** : `--log-gestures <fichier.json>` journalise chaque geste exécuté (trajectoire + pauses) ; `gesture_playback.py replay <nom>` les rejoue à l'identique (boucle, vitesse, jitter optionnel).
- **Scroll avec TES gestes** : `--gestures-file <fichier.json>` remplace les swipes générés par TON style (gestes enregistrés ≥ 120 px, rejoués via `motionevent`, choisis au hasard, léger facteur de vitesse) — la cadence (2,5 s min, avant-fin) et les interactions restent gérées par le warmup. Pool actuel : `gestures/mon_style.json` (session de 60 s — 21 swipes). Pour l'enrichir : `gesture_playback.py record mon_style --seconds 60` puis scroller.
- **Carousels (IG & TikTok)** : le script tire `--carousel-prob` à chaque swipe ; si déclenché, il fait de 1 à 3 petits slides **à gauche** (`adb shell input swipe droite→gauche`), légèrement de travers, avant le swipe vertical — sur Instagram comme sur TikTok. Sur une vidéo simple ce geste latéral est inoffensif ; sur un carousel il révèle les photos suivantes.
- **Détour profil / stories** : le script tire `--profile-prob` après un swipe ; si déclenché, il tape sur l'avatar puis détecte (dump UI) : viewer de stories déjà ouvert (`Like Story`) → regarde 1–4 stories (taps à droite, pauses 2,5–6 s) et ferme par `keyevent 4` (le viewer revient aux reels) ; profil ouvert → parcours éventuel + rangée de stories cliquée parfois (`--story-prob`), puis retour par la flèche calibrée. Si le viewer était déjà ouvert, pas de retour supplémentaire (évite de sortir du lecteur Reels).
- **Like** : le script tire `--like-prob` (cadence min `LIKE_COOLDOWN`) ; si déclenché, double-tap à droite de la vidéo (60 % des cas, robuste) ou tap sur le cœur réglable par app. Le résumé de fin affiche les likes par app.
- **Commentaires** : le script tire `--comments-prob` (cadence min `COMMENTS_COOLDOWN`) ; si déclenché, tap sur le bouton commentaire (`APP_DEFS[app]["comments"]`, réglable), 2–5 swipes lents pour lire, avec **arrêts de lecture** (pause 2,5–5,5 s sur un commentaire, jusqu'à 2 par session, parfois like du commentaire) **suivis immédiatement d'un scroll** (jamais d'arrêt sur le dernier swipe), puis fermeture par swipe vers le bas. Le résumé affiche les sessions par app.
- **Un seul geste continu par vidéo** : jamais de segmentation en plusieurs `input swipe` — chaque segment serait un défilement à part (2–4 vidéos d'un coup, constaté en réel et corrigé).
- `MSYS_NO_PATHCONV=1` est géré par le script (Git Bash Windows) — ne pas l'omettre si on envoie des commandes ADB à la main.
- Le script s'arrête **proprement** si le téléphone se débranche en cours de route.
- En cas de doute sur l'état de l'écran pendant le warmup : `adb shell uiautomator dump /data/local/tmp/w.xml` puis lecture du fichier — sans interruption du défilement.

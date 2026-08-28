# Dashboard Social de Switch développement

Ce sidecar connecte des comptes Instagram professionnels et TikTok via leurs
OAuth officiels. Il stocke les jetons chiffrés, sépare les données par utilisateur
Switch et affiche les vues quotidiennes par compte sur un graphique 2D, avec les
détails par Reel ou vidéo lorsque le fournisseur expose un compteur exact.

## Onglet « Réseaux sociaux » dans l'application

Le dashboard est aussi accessible comme **onglet natif** de l'application
(barre latérale → Suivi → **Réseaux sociaux**, et menu mobile) : il charge la
page `/social/` dans une iframe same-origin, la conserve entre les re-rendus,
et ouvre automatiquement l'onglet au retour d'un OAuth Instagram/TikTok
(`/?switch_social=connected`). L'ancien loader injecté
(`public/social-loader.js`) qui ouvrait le dashboard en dialogue a été
remplacé par cet onglet.

## Configuration locale

1. Copier `social-analytics/social.env.example` vers
   `social-analytics/social.env`.
2. Copier `social-gateway/gateway.env.example` vers
   `social-gateway/gateway.env`.
3. Générer une valeur aléatoire d'au moins 32 octets pour `SOCIAL_PROXY_KEY` et
   mettre exactement la même valeur dans les deux fichiers locaux.
4. Générer et conserver `SOCIAL_TOKEN_ENCRYPTION_KEY`, puis renseigner les
   identifiants d'application Meta et TikTok.
5. Enregistrer exactement les URI de retour indiquées dans le modèle auprès des
   fournisseurs. Elles ciblent `pc-fixe` et jamais le VPS Azure.

Les fichiers `social.env` et `gateway.env` sont ignorés par Git. Les conteneurs
Social n'exposent aucun port hôte ; seule la façade loopback de développement
route `/social/` et `/api/social/` vers eux.

## Sémantique des données

- Instagram : les vues journalières viennent de l'API Insights d'un compte
  Business ou Creator ; les vues de Reels sont lues individuellement.
- TikTok : `view_count` est cumulatif. Une valeur journalière n'est calculée que
  lorsque deux inventaires complets ont été observés à 18–30 heures d'intervalle.
  Une vidéo ajoutée, absente ou sans compteur rend la journée indisponible au
  lieu d'inventer une valeur partielle.
- Les plages 7, 30 et 90 jours utilisent les dates civiles `Europe/Paris`, y
  compris les journées de 23 et 25 heures lors des changements d'heure.

Le mode démonstration est désactivé par défaut. Une déconnexion TikTok tente de
révoquer le jeton fournisseur avant de supprimer localement le compte ; une panne
temporaire conserve le compte afin de permettre une nouvelle tentative.

## Vérifications source

Sans démarrer Docker, les contrôles du module sont :

```text
node --check social-analytics/server.mjs
node --check social-gateway/proxy.mjs
node --test social-analytics/test/*.test.mjs social-gateway/test/*.test.mjs
```


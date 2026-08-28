# Microsoft 365 dans les chats : e-mails et agenda

Une fois un compte Microsoft 365 lié, un chat normal peut lire la boîte Outlook et l’agenda de l’utilisateur connecté, puis préparer un e-mail ou un rendez-vous que celui-ci confirme d’un clic dans la conversation. Codex Switch Terminal utilise l’API officielle **Microsoft Graph** avec un flux OAuth 2.0 délégué : aucun mot de passe Microsoft n’est demandé ni conservé.

## Prérequis Entra ID

1. Dans [Microsoft Entra ID → Inscriptions d’applications](https://entra.microsoft.com/), créer une inscription, par exemple « Codex Switch Terminal ».
2. Choisir les types de comptes pris en charge : un seul annuaire pour un locataire d’entreprise, ou « comptes dans un annuaire organisationnel et comptes personnels Microsoft » pour accepter aussi les adresses Outlook.com.
3. Ajouter une URI de redirection de type **Web** strictement identique à celle du serveur, par exemple `https://switch.example.com/api/microsoft/callback`. Entra compare la chaîne caractère par caractère : ni barre oblique finale, ni port implicite, ni domaine voisin.
4. Dans **Certificats et secrets**, créer un **secret client** et copier immédiatement sa *valeur*, pas son identifiant ; elle n’est plus affichée après le rechargement de la page.
5. Dans **API autorisées → Microsoft Graph → Autorisations déléguées**, ajouter `User.Read`, `Mail.Read`, `Mail.Send`, `Calendars.ReadWrite` et `offline_access`. Les portées `openid`, `profile` et `email` sont demandées avec elles.
6. Si le locataire impose une validation centrale, cliquer sur **Accorder le consentement administrateur**. Sinon chaque utilisateur consent lui-même au premier passage sur l’écran Microsoft.

Relever enfin l’**ID d’application (client)** et, pour un locataire unique, l’**ID d’annuaire (locataire)**, appelé *tenant* dans le portail en anglais.

## Configurer depuis l’application (recommandé)

Le plus simple est de saisir les identifiants Entra directement dans l’application, sans toucher au serveur ni redéployer. Ouvrir **Mon compte → Microsoft 365** : tant que rien n’est configuré, un formulaire demande l’**ID d’application (client)**, le **secret client** et le **tenant**. Il affiche aussi l’**URI de redirection** à déclarer dans Entra — copiez-la avant de créer l’inscription. Une fois enregistrée, la configuration est persistée sur le serveur dans `<CST_DATA_DIR>/microsoft-provider.json` (`0600` sous Linux) et **survit aux redéploiements**, contrairement aux variables d’environnement que le playbook réécrit. Le bouton **Reconfigurer** permet d’en changer ; le secret n’est jamais renvoyé à l’interface.

Cette configuration s’applique à tout le nœud : dans un réseau fermé et de confiance, n’importe quel utilisateur connecté peut la saisir. La configuration par formulaire l’emporte sur les variables d’environnement si les deux sont présentes.

## Variables d’environnement du serveur (alternative)

À défaut du formulaire, les identifiants peuvent venir des variables du nœud : `deploy/cst-server.env.example` pour une installation native, le fichier d’environnement du conteneur pour un déploiement Docker Compose.

```bash
CST_MICROSOFT_CLIENT_ID=00000000-0000-0000-0000-000000000000
CST_MICROSOFT_CLIENT_SECRET=valeur-du-secret-client
# Facultatif : "common" par defaut, a remplacer par l'ID du locataire pour
# limiter la connexion a une seule organisation.
CST_MICROSOFT_TENANT_ID=common
# Facultatif : <CST_PUBLIC_BASE_URL>/api/microsoft/callback par defaut.
CST_MICROSOFT_REDIRECT_URI=https://switch.example.com/api/microsoft/callback
# Facultatif : ne changer que pour retirer une capacite.
CST_MICROSOFT_SCOPES="offline_access openid profile email User.Read Mail.Send Mail.Read Calendars.ReadWrite"
```

L’identifiant et le secret vont ensemble : tant que les deux ne sont pas présents, l’intégration reste éteinte et l’application indique simplement qu’elle n’est pas configurée sur ce serveur. En dehors de la boucle locale, l’URI de redirection doit être en HTTPS ; sinon l’intégration se désactive au démarrage avec un message sur la sortie d’erreur, sans empêcher le nœud de démarrer.

## Liaison du compte dans l’application

La liaison est nominative : elle appartient au compte utilisateur connecté, pas à l’installation.

1. Se connecter au serveur avec son compte utilisateur.
2. Cliquer sur son nom en haut de l’application pour ouvrir **Mon compte**, puis descendre jusqu’à la carte **Microsoft 365**.
3. Cliquer sur **Connecter Microsoft 365** : le navigateur quitte l’application vers l’écran de connexion Microsoft.
4. S’authentifier, puis accepter les autorisations demandées.
5. Microsoft renvoie sur l’application, qui rouvre **Mon compte** et affiche l’adresse liée, le nom du compte et la date de liaison.
6. **Délier**, puis **Confirmer la déliaison**, supprime la connexion et ses jetons du serveur.

La liaison vit dans **Mon compte** et non dans **Paramètres** parce qu’elle est nominative : elle suit la personne, pas l’installation ni le serveur. **Paramètres** n’affiche qu’un raccourci vers cette carte, avec l’état courant.

### Plusieurs boîtes

Vous pouvez lier plusieurs comptes Microsoft — une boîte personnelle et une boîte professionnelle, par exemple. **Ajouter une autre boîte** relance la connexion ; choisissez un autre compte sur l’écran Microsoft pour l’ajouter à côté du premier. La liste dans **Mon compte** affiche chaque boîte, avec :

- **Par défaut** : la boîte utilisée quand une demande ne précise pas laquelle. **Définir par défaut** la change ; la première boîte liée l’est automatiquement.
- **Délier** (une boîte) ou **Délier toutes les boîtes** : efface les jetons du serveur.

Depuis un chat, la boîte par défaut sert sauf indication contraire. Dites simplement « depuis ma boîte pro » et le modèle vise cette adresse ; il ne peut choisir qu’entre **vos** boîtes liées, jamais une adresse tierce. Sur la carte de confirmation d’un envoi, la ligne **De** rappelle l’expéditeur et, si vous avez plusieurs boîtes, permet d’en changer avant de valider.

Si Microsoft révoque l’autorisation d’une boîte — mot de passe changé, consentement retiré, longue inactivité —, elle passe à l’état **À relier** et propose **Relier**. Ses jetons sont effacés mais son adresse reste visible ; vos autres boîtes continuent de fonctionner.

Si le serveur est ouvert par `127.0.0.1` derrière un tunnel, le départ de la liaison bascule automatiquement sur l’origine publique déclarée dans Entra : c’est la seule URL que Microsoft acceptera.

## Envoyer un message dans un chat (canal `send_chat_message`)

Le modèle peut poster un message dans un chat **déjà ouvert** de l’interface
web (y compris le chat courant), sans toucher à l’écran ni au focus : le canal
passe par l’API interne, pas par la souris ou le clavier. Il est **désactivé par
défaut** et ne s’expose au modèle que si le serveur est lancé avec
`CST_AGENT_CHAT_POST=1`. Quand il est actif, le chat cible traite le message
comme un message normal de l’utilisateur, avec le compte et le modèle de CE
chat.

- Cible : `chat` absent ou `"current"` pour le chat courant du modèle ;
  sinon identifiant de session d’un autre chat déjà ouvert.
- Garde-fous : les mots de passe, codes de vérification et données bancaires
  sont refusés (saisis par l’utilisateur) ; le modèle ne doit pas s’envoyer de
  message à lui-même en boucle ; file bornée à 32 demandes avec expiration
  (2 h).
- Le message envoyé est un vrai message utilisateur du chat cible : il déclenche
  le traitement normal (tour du modèle de ce chat), comme si l’utilisateur
  l’avait tapé.

## Utilisation depuis un chat

Cinq outils sont exposés au modèle. Chacun accepte un champ `account` facultatif pour viser l’une de vos boîtes ; sans lui, la boîte par défaut est utilisée. Deux lisent immédiatement :

- `list_outlook_messages` liste jusqu’à 25 messages de `inbox`, `sentitems`, `drafts` ou `archive`, avec une recherche plein texte facultative. Les corps complets ne sont pas retournés : l’objet, l’expéditeur et l’aperçu suffisent à résumer une boîte ;
- `list_calendar_events` liste jusqu’à 50 événements, occurrences des séries récurrentes comprises.

Trois autres ne font que **proposer** : `send_outlook_email`, `create_calendar_event` et `update_calendar_event` déposent une carte de confirmation dans la conversation. Rien ne part et rien n’est écrit tant que l’utilisateur n’a pas cliqué sur le bouton d’envoi de cette carte. Le modèle est instruit de ne jamais annoncer un e-mail parti ou un rendez-vous créé, seulement une proposition en attente.

La carte survit à un rechargement de page et reste valable six heures. Elle affiche le brouillon complet — destinataires, objet, corps, créneau converti dans le fuseau local — et propose aussi de l’annuler.

### Connecter son compte sans quitter la conversation

Si un outil réclame la boîte ou l’agenda alors qu’aucun compte n’est lié, une carte **Compte Microsoft à connecter** apparaît directement dans le fil, avec le bouton de liaison. Elle sert aussi quand Microsoft a révoqué l’autorisation, et explique la situation sans proposer de bouton quand le serveur n’a pas de configuration Entra.

Un e-mail rédigé avant la liaison **n’est pas perdu** : le brouillon est conservé, son envoi reste fermé, et il redevient confirmable dès le compte connecté. La proposition de liaison s’efface d’elle-même une fois le compte lié, expire au bout d’une heure, et peut être masquée à la main.

## Utilisation depuis un agent autonome

Un agent autonome créé par un utilisateur connecté hérite de son propriétaire et reçoit les mêmes cinq outils, **et uniquement ceux-là** : il ne peut ni créer d’autres agents, ni ouvrir des chats. Un agent créé avant cette version, ou avec le jeton administrateur, n’a aucun propriétaire et n’obtient donc aucun accès — il faut le recréer depuis un compte connecté.

Un agent travaille sans personne devant l’écran : ses e-mails et ses rendez-vous rejoignent la même file de confirmation. Une pastille sur le bouton de compte indique le nombre d’actions en attente, et **Mon compte** les liste avec leur contenu complet. Sans validation, elles expirent au bout de six heures.

Deux limites du moteur s’appliquent : les cycles de *planification* d’un agent en validation humaine n’ont pas les outils, et une session Claude persistante désactive le serveur d’outils MCP.

La boîte et l’agenda sont ceux du compte lié : il est inutile de préciser une adresse d’expéditeur, un identifiant ou une boîte cible, et le modèle est instruit de n’inventer aucune adresse de destinataire.

## Sécurité et fonctionnement

- Les jetons d’accès et de renouvellement sont conservés côté serveur dans `<CST_DATA_DIR>/microsoft-connections.json`, **en clair**, protégés par les seules permissions du fichier : `0600` sous Linux. Sous Windows, ce durcissement n’est pas appliqué, exactement comme pour `user-auth.json`.
- Si la configuration Entra est saisie dans l’application, l’identifiant client, le tenant et le secret client sont stockés dans `<CST_DATA_DIR>/microsoft-provider.json` avec la même protection (`0600`, secret en clair). Le secret n’est jamais renvoyé par l’API — l’interface ne connaît que l’identifiant client (public). Comme la configuration s’applique au nœud entier, ce mode suppose un réseau fermé et des utilisateurs de confiance.
- Un jeton de renouvellement Entra donne accès à la boîte et à l’agenda pendant environ 90 jours. Quiconque lit ce fichier lit les e-mails et l’agenda du compte lié. Il faut donc protéger le dossier de données comme un secret de production, et délier le compte depuis l’application dès qu’on n’en a plus besoin.
- Les jetons ne sont jamais renvoyés à l’interface ni exposés par l’API : celle-ci ne publie que l’adresse liée, le nom affiché, les portées et la date de liaison.
- Le flux utilise `state` et PKCE, et le retour de Microsoft est traduit en cinq codes internes (`cancelled`, `invalid`, `conflict`, `session`, `failed`). Les messages bruts du fournisseur ne sont volontairement pas propagés : ils exposeraient le locataire et l’identifiant client dans l’URL de retour.
- La déliaison et chaque confirmation d’action exigent un en-tête de confirmation dédié, en plus de la session : un lien ouvert par erreur ou une page tierce ne peuvent ni envoyer un e-mail ni effacer une liaison.
- Un même compte Microsoft ne peut être lié qu’à un seul compte utilisateur du serveur.

## Limites connues

- La liaison est locale à un nœud : les comptes vivent dans le `CST_DATA_DIR` de chaque serveur, et une liaison faite sur un nœud n’existe pas sur les autres.
- Chaque nœud a donc sa propre URI de redirection, à déclarer dans la même inscription Entra.
- Aucun envoi et aucune écriture d’agenda n’ont lieu sans un clic humain sur la carte de confirmation : une conversation laissée seule n’enverra jamais rien, et une action non confirmée disparaît au bout de six heures.
- Pas de pièces jointes : le corps des requêtes d’outil est plafonné à 64 Kio. Les e-mails sont envoyés en texte brut.
- Les dates transmises au modèle doivent porter un décalage horaire explicite, par exemple `2026-07-25T14:00:00+02:00` ; un instant sans décalage est refusé. Les horaires renvoyés par l’agenda sont en UTC.
- `update_calendar_event` modifie un événement existant mais ne le supprime ni ne l’annule ; un déplacement d’horaire exige le début et la fin ensemble.
- Un tour de conversation ne peut préparer que trois actions externes, et un compte ne conserve pas plus de vingt actions en attente.

La référence officielle des ressources utilisées est la [documentation Microsoft Graph](https://learn.microsoft.com/graph/overview), et le flux de consentement est décrit dans [Plateforme d’identités Microsoft — code d’autorisation OAuth 2.0](https://learn.microsoft.com/entra/identity-platform/v2-oauth2-auth-code-flow).

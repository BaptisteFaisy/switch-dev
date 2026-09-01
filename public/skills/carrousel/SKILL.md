---
name: carrousel
description: Compose, review or improve the content of an image carousel (multi-image post) — hooks, per-slide titles, captions, calls to action and publication order. Use when the user shares carousel images or data from the Studio IA Carrousel tab, or asks to write carousel copy.
---

# Carrousel

Un carrousel est une publication multi-images (Instagram, LinkedIn, TikTok carousel…) : chaque diapositive combine une image et un court texte, et la dernière porte généralement l'appel à l'action. Le Studio IA de Switch peut fournir la structure complète : nom du carrousel, puis par diapositive un titre, un texte, un libellé de bouton et un lien.

## Quand ce skill s'applique

- L'utilisateur envoie un carrousel depuis le Studio IA (les images jointes et la data texte accompagnent le message).
- L'utilisateur demande d'écrire, réécrire, améliorer ou critiquer le contenu d'un carrousel, même sans données structurées.
- L'utilisateur veut un plan de publication ou des variantes d'accroches.

## Règles de composition

1. **Accroche dès la première diapositive.** Le titre doit arrêter le scroll en 1 à 2 secondes : promesse claire, chiffre, ou question ciblée. Chaque diapositive suivante doit donner envie d'avancer.
2. **Une idée par diapositive.** Si une diapositive contient plusieurs messages, la scinder ou choisir l'idée la plus forte.
3. **Ton et registre constants** d'une diapositive à l'autre (tu/vous, tutoiement, vocabulaire). Respecter la marque ou le style demandé.
4. **Textes courts.** Titre : 3 à 10 mots. Texte : 1 à 3 phrases. Ne jamais dupliquer le texte visible dans l'image : le texte écrit complète l'image, il ne la décrit pas.
5. **Appel à l'action cohérent.** La dernière diapositive doit reprendre le fil et proposer une action unique et claire (bouton/lien fourni dans la data, sinon suggérer un CTA réaliste).
6. **Ordre narratif.** Vérifier que l'ordre des diapositives raconte une histoire : problème → explication → preuve → action. Signaler un ordre faible et proposer un meilleur enchaînement.

## Utilisation des données du Studio IA

Quand l'utilisateur fournit une structure (`Diapositive N · Titre : … · Texte : … · Bouton : … · Lien : …`), respecter les champs existants : conserver les liens et libellés de bouton sauf demande contraire, et réécrire uniquement ce qui est demandé. Préciser toujours ce qui a été modifié et pourquoi.

## Livrable

Renvoyer le contenu complet du carrousel, diapositive par diapositive (titre, texte, bouton, lien), prêt à être recopié dans le Studio IA. Si les images sont jointes, aligner chaque texte sur ce que montre l'image correspondante. Proposer ensuite 1 à 2 variantes d'accroche si demandé, ou un plan de publication.

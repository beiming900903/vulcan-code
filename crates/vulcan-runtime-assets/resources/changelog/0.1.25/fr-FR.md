Vulcan Code 0.1.25 améliore le démarrage de l’application, les bases de connaissances et les assistants dédiés. Le programme d’installation autonome 0.2.13 permet de choisir la source de téléchargement.

### Démarrage et exécution

```ui-checklist
ok | Progression au démarrage | L’écran affiche l’étape en cours de Core et le nom de la compétence en cours de téléchargement.
ok | Compatibilité LuaSkills | Core utilise l’environnement Lua et les chemins de modules fournis, à l’abri des variables héritées. Le chargement des modules fonctionne aussi depuis les chemins d’installation Windows avec des caractères non ASCII.
```

### Bases de connaissances

```ui-checklist
ok | Réutilisation des index | Le changement de mode de recherche conserve les index et les paramètres de vectorisation. Si le même espace vectoriel est réactivé, seul le contenu manquant est traité.
ok | Aperçu et recherche | L’application web affiche un aperçu Markdown des documents actuels ou historiques et permet de voir la source. Un résumé ouvre le début du document, tandis qu’un résultat textuel ouvre la ligne correspondante.
```

### Assistants et modèles

```ui-checklist
ok | Mémoire verrouillée | La conversion ouvre l’assistant flottant et affiche l’état du verrouillage. Les tours terminés sous verrouillage signalent que leur contenu ne sera pas transmis au tour suivant.
ok | Instructions système | Les modifications des instructions de base s’appliquent dès la prochaine exécution, avant l’historique, sans actualiser la session.
```

### Programme d’installation autonome 0.2.13

```ui-checklist
ok | Sources de téléchargement | Choisissez Automatique, International ou Chine continentale. Le choix est conservé et peut aussi être défini avec `--route`.
```

Vulcan Code 0.1.25 verbessert den Desktopstart, Wissensdatenbanken und spezialisierte Assistenten. Das eigenständige Installationsprogramm 0.2.13 bietet eine Auswahl an Downloadquellen.

### Start und Laufzeit

```ui-checklist
ok | Startfortschritt | Der Startbildschirm zeigt die aktuelle Core-Phase und nennt den Skill, der gerade heruntergeladen wird.
ok | LuaSkills-Kompatibilität | Core nutzt die mitgelieferte Lua-Laufzeit und deren Modulpfade, damit geerbte Umgebungsvariablen nicht stören. Module lassen sich unter Windows auch aus Installationspfaden mit Nicht-ASCII-Zeichen laden.
```

### Wissensdatenbanken

```ui-checklist
ok | Indizes wiederverwenden | Beim Wechsel des Suchmodus bleiben Indizes und Embedding-Einstellungen erhalten. Wird derselbe Vektorraum wieder aktiviert, werden nur fehlende Inhalte ergänzt.
ok | Vorschau und Suche | Die Web-App zeigt aktuelle und ältere Dokumente standardmäßig als Markdown-Vorschau mit umschaltbarem Quelltext. Zusammenfassungen öffnen den Dokumentanfang, Texttreffer die passende Zeile.
```

### Assistenten und Modelle

```ui-checklist
ok | Gesperrter Speicher | Nach der Umwandlung öffnet sich der schwebende Assistent und zeigt den Sperrstatus an. Unter der Sperre abgeschlossene Runden sind gekennzeichnet, da deren Inhalte nicht in die nächste Runde übernommen werden.
ok | Systemanweisungen | Änderungen an den Basisanweisungen werden bei der nächsten Ausführung vor dem Verlauf berücksichtigt. Die Sitzung muss dafür nicht neu geladen werden.
```

### Eigenständiges Installationsprogramm 0.2.13

```ui-checklist
ok | Downloadquellen | Beim Installieren oder Aktualisieren stehen Automatisch, International und Festlandchina zur Wahl. Die Auswahl wird gespeichert und kann mit `--route` gesetzt werden.
```

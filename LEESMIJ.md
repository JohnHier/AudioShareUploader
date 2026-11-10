# AudioShare Uploader 0.2.0

Uploadt programma's automatisch naar AudioShare vanaf een server-pc in de studio: op een vaste tijd, of zodra een bestand vernieuwd is. Draait als Windows-service en wordt ingesteld via een webinterface.

## Installeren

1. In AudioShare (admin): **Recorders → Studio's → Studio aanmaken**, bijv. "Server Studio 2". Vink de programma's aan die deze server mag aanleveren en klik op **Alleen koppelcode**.
2. Kopieer `AudioShareUploader.exe` naar de server-pc, bijv. `C:\AudioShareUploader\`.
3. Open een opdrachtprompt **als administrator** in die map en voer uit: `AudioShareUploader.exe install`.
4. Open op de server-pc zelf `http://localhost:8470` en maak het beheeraccount aan (wachtwoord minimaal 10 tekens).
5. **Instellingen → Koppeling met AudioShare**: vul het adres en de koppelcode in (of kies het `.audioshare`-bestand) en klik op Koppelen.
6. **Taken → Taak toevoegen**.

Daarna is de webinterface bereikbaar vanaf andere pc's in het netwerk op `http://<naam-of-ip-van-server>:8470`.
Sta in de Windows Firewall poort 8470 (TCP) toe voor het lokale netwerk.

## Een taak

| Instelling | Uitleg |
|---|---|
| Programma | Uit de lijst die AudioShare voor deze studio toestaat |
| Bron | Een vast bestand, of een map (dan het nieuwste audiobestand: MP3, WAV, FLAC, OGG). Netwerkpaden (`\\nas\audio\...`) kunnen, mits de service er toegang toe heeft (zie hieronder) |
| Moment | **Bewaken**: uploaden zodra het bestand nieuw of veranderd is. **Vaste tijd**: op gekozen dagen en tijdstip |
| Wachttijd | Een bestand moet zo lang ongewijzigd zijn (standaard 60 s), zodat een half weggeschreven opname niet meegaat |
| Uitzenddatum | Vandaag, morgen, eerstvolgende uitzenddag (vandaag telt mee), of uit de bestandsnaam (`2026-11-14`, `20261114`, `14-11-2026`) |
| Titel | Sjabloon met `{programma}`, `{datum}`, `{dag}`, `{bestand}` |

- Bij een nieuwe taak (of een andere bron) geldt het bestand dat er nu staat als al verstuurd. Wil je het toch uploaden: **Nu uploaden**.
- Staat er al een aflevering op die datum, dan wordt die vervangen.
- Een bestand met dezelfde inhoud wordt niet nog eens voor dezelfde datum geüpload.
- Voldoet een bestand niet aan de normen (WAV, FLAC, OGG, of een MP3 met lagere bitrate), dan zet de Uploader het om naar MP3 320 kbps. Het origineel wordt nooit veranderd, verplaatst of verwijderd.
- Een geplande upload die meer dan 6 uur gemist is (server uit), wordt overgeslagen en in het logboek gezet.
- Bij elke aflevering staat in AudioShare "bediend door: Automatische upload (taaknaam)".

## Meldingen

De Uploader meldt problemen aan AudioShare (vanaf versie 1.6.0) en AudioShare mailt ze naar het meldingsadres uit de platforminstellingen: mislukte upload, een uur lang geen verbinding, geen nieuw bestand op een vaste tijd, en een bron die niet gevonden wordt (max. 1 keer per dag). Alles staat ook in het **Logboek**.

## Beveiliging

- Koppeling als **studio**: de sleutel geeft alleen toegang tot de programma's van die studio en is in AudioShare met één klik in te trekken. Op de server staat hij in Windows Credential Manager (versleuteld door Windows).
- Webinterface: inloggen met gebruikersnaam en wachtwoord (Argon2id), sessie van 12 uur, na 5 foute pogingen 5 minuten wachten. Het eerste account kan alleen op de server zelf worden gemaakt.
- Standaard alleen bereikbaar vanaf het lokale netwerk. Zet toegang van buiten pas aan als het via een VPN of een beveiligde (HTTPS-)proxy loopt; de webinterface zelf spreekt gewoon HTTP.

## Netwerkmappen (NAS)

De service draait als *Lokaal systeem*. Die ziet geen netwerkschijven van een gebruiker (zoals `Y:`) en kan niet zelf inloggen op een NAS. Daarom logt de Uploader zelf in:

1. **Instellingen → Netwerkmappen**: vul de schijfletter in (bijv. `Y`), het netwerkpad (bijv. `\\NASBR6\Mediafiles_Playout`) en een gebruikersnaam en wachtwoord van de NAS.
2. Klik op **Opslaan en testen**. Je ziet meteen of inloggen en lezen lukt.
3. In de taken kun je daarna gewoon `Y:\muziek\...` gebruiken; de Uploader maakt daar zelf `\\NASBR6\Mediafiles_Playout\muziek\...` van.

Het netwerkpad achter een schijfletter vind je met `net use` in een opdrachtprompt, of in Verkenner achter de schijfnaam. Het wachtwoord staat in Windows Credential Manager (versleuteld door Windows).

## Bijwerken naar een nieuwe versie

In een opdrachtprompt **als administrator**:

```
sc stop AudioShareUploader
copy /Y <nieuwe exe> C:\AudioShareUploader\AudioShareUploader.exe
sc start AudioShareUploader
```

Taken, koppeling en instellingen blijven bewaard.

## Beheer

- Gegevens: `C:\ProgramData\AudioShareUploader\` (`settings.json`, `tasks.json`, `uploads.json` met wachtrij en logboek, `werk\` voor omgezette kopieën).
- Testen zonder service: `AudioShareUploader.exe run`.
- Verwijderen: `AudioShareUploader.exe uninstall` (gegevens blijven staan).
- Een omzetting gebeurt één tegelijk; een lange WAV omzetten duurt even, andere taken wachten zo lang.

## Techniek

Rust. Upload-, omzet- en beveiligingscode gedeeld met de Studio Recorder (`src/audioshare`, `decode.rs`, `export.rs`, `wav.rs`). `rules.rs`: bron, datum, titel en planning (met tests). `engine.rs`: de lus. `web.rs` + `web/`: de webinterface. Bouwen voor Windows: `build-win.sh` (cross-compile) of `cargo build --release` op Windows.

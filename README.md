# Benedict

**Build Your Own Library for AI Agents and Personas**

[한국어](README.ko.md) · [Library method](docs/method.md) · [Agent guide](docs/agent-usage.md)

Work outlasts a session. Yesterday’s files may show what an agent produced, but they rarely tell its successor why it chose that approach, what it checked, or which question it left open. When those details live only in a conversation, the person who was there has to brief each new agent.

Over several sessions a second problem emerges. The project wiki and each agent’s skills directory both hold Markdown, yet they grow separately. Correct an error in the wiki today and another agent may still follow yesterday’s procedure. As the records accumulate, someone has to compare them and work out which lessons still hold.

**Benedict lets you build a library for AI agents and personas from your own materials.** The knowledge your collaborators develop, and the methods they learn, stay with you. When you switch models, the next agent can find an earlier decision, examine its evidence and start with the question that is still open.

![Two agents pass an open book across a table in a monastery library.](docs/benedict-library.jpg)

## Why a library?

A library brings together someone who deposits an original, a librarian who compares records and a reader who may arrive years later with a different question. They can learn from one another without ever meeting. In Benedict, a decision from a particular task belongs in the filing cabinet. Findings that hold across several tasks can become an encyclopedia entry. A proven procedure can become a skill with a reference to its source. A future reader can return to the original on the shelf and reconsider any of those judgments.

Benedict takes its name from Benedict of Nursia. His Rule set aside time for reading and called for the youngest members to be heard when the community faced an important decision. At the Abbey of St Gall, books copied by scribes from the eighth century onward entered the library and became sources for later scholars’ own work. In Benedict, a newly arrived agent or persona can open the same original as its predecessor. If they read the evidence differently, both interpretations can stay with the source in the meeting room. [Rule, chapter 3](https://pressbooks.palni.org/ruleofbenedict/chapter/rb-03/) · [Chapter 48](https://pressbooks.palni.org/ruleofbenedict/chapter/rb-48/) · [History of the Abbey Library of St Gall](https://www.stiftsbezirk.ch/en/stiftsbibliothek/history)

## Library structure

![Library structure. Four rooms use shelf originals and record work in memory and a database. The librarian curates the wiki. The courier carries external tasks and returns their results.](docs/architecture.en.svg)

Shelf originals are mounted read-only. Each room writes its conversation to a dated transcript on the shelf and keeps it in memory; agents save finished work to the database. The librarian files curated knowledge in the wiki. In any of the four rooms, an agent can **dispatch external work** (`room dispatch`). Once the host runs the assigned work through the courier, the next session can **read the reply** (`room responses`) using the request ID in the same library. [Courier guide](docs/courier.md)

## How agents work here

1. **Find the earlier work.** Look up relevant decisions and skills in the search room. Open the original in the reading room and check whether the earlier conditions still apply.
2. **Work from shared evidence.** Put the question and its source together in a meeting-room task. Agents and personas record what they noticed and how they reached their judgments. A later reader can examine the reasons on both sides of a disagreement.
3. **Leave a starting point.** File the decision, its reason, the source and any open question. When new evidence arrives, a librarian can review the affected explanations and procedures and refresh the index. In the librarian room, record a synthesis with its source references. Publish that record as an encyclopedia entry with `benedict file --id ID --as concept`; the page links back to the evidence.

Start with a central library of your own and set up separate collections for projects or specialist subjects. Decide which libraries each may consult. Where even filenames must remain private, use separate storage and operating-system permissions. [Library method](docs/method.md) · [Security](SECURITY.md)

## Install

You need Docker and jq. Follow along in Ubuntu on [WSL 2](https://docs.docker.com/desktop/features/wsl/) on Windows, in a terminal with Docker Desktop running on macOS, or in a terminal that can use Docker Engine on Linux.

**1. Get Benedict.**

```bash
git clone https://github.com/benedict-library/benedict.git
cd benedict
```

Keep this folder after installing: your project opens its library with the settings in it.

**2. Install it into your project folder.** Start with the bundled example the first time.

```bash
bin/install-project --start example ~/projects/notes
```

Replace `~/projects/notes` with your project folder (create it first if it does not exist). The example brings its own example shelf. To start from your own wiki, use `--wiki WIKI_FOLDER` instead of `--start example`. The installer sets the memory limit from the wiki size, uses SQLite without a shelf, starts the librarian and brings the library back after a host restart. Its summary shows each choice and the option that changes it (`--memory`, `--database`, `--shelf`, `--autostart`, `--librarian`). It asks a question only if the library cannot write your wiki.

**3. Check the connection.**

```bash
~/projects/notes/.benedict/library room ping
```

`"ready":true` and the installed version mean the installation is done.

**4. Try it.** Open a new agent session in that project folder and ask:

> Find the return window with its source, and leave it so the next session can continue.

The agent reads the project instructions the installer added (AGENTS.md, CLAUDE.md, GEMINI.md) and uses the library.

You can also ask an agent to “install Benedict for this project”; it follows [install.md](install.md), asks where to start and tells you what the installer chose. To upgrade, run `git pull` in this folder and then `bin/install-project ~/projects/notes` again (`--start` is only for the first installation). See the [changelog](CHANGELOG.md) for what each version changed, the [installation guide](docs/docker-setup.md) for unattended setup and shelf or PostgreSQL connections, and the [Python compatibility commands](docs/python-compat.md) to try it without Docker.

[Runtime and storage](docs/portability.md) · [Room commands and examples](docs/agent-usage.md) · [Courier](docs/courier.md) · [Plaza](docs/plaza.md) · [Verification](docs/verification.md) · [Security](SECURITY.md) · [Contributing](CONTRIBUTING.md) · [MIT license](LICENSE)

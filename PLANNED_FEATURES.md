# Planned Features

This document outlines features currently planned for future versions of **Knot**.

These features represent improvements and ideas that are actively being considered for development. Some are smaller quality-of-life additions, while others are larger upgrades that will take more time to build.

Features listed here are part of the long-term direction of Knot, but may be added gradually across multiple releases.

---

## Project Starter Template

Starting a new project should feel less like opening a blank text file and more like actually starting a Twine project.

Future versions of Knot will be able to generate a complete starter project when creating a new workspace. Instead of manually setting everything up, users will be given a ready-to-use project structure with the essential passages already in place.

Planned improvements include:

* Automatic creation of story metadata passages
* A generated story identifier for the project
* A starter `Start` passage so writing can begin immediately
* Built-in reference files containing commonly used special passages
* Optional workspace configuration files with recommended defaults

This is intended to make onboarding easier for new users and reduce repetitive setup work.

**Status:** Planned

---

## Move Passage to New File

Writers often begin drafting multiple passages inside a single file and organize things later.

A future update will allow passages to be moved directly into their own files without manually copying and restructuring content.

Planned functionality includes:

* Moving the currently selected passage into a new file
* Automatically removing the original passage from the old file
* Choosing a custom filename during the move process
* Reducing manual copy-paste when reorganizing larger stories

This should make reorganizing projects much faster as stories begin to grow.

**Status:** Planned

---

## Passage Organization Tools

As projects become larger, manually managing dozens or even hundreds of passages becomes increasingly difficult.

Knot is planned to include more advanced organizational tools to help authors restructure projects more easily without manually editing files.

Planned improvements include:

* Moving passages between existing files
* Splitting large files into smaller files
* Merging multiple files together
* Bulk passage management tools
* Better visual organization through the Story Map

The long-term goal is to make managing large Twine projects significantly easier.

**Status:** Long-term planned feature

---

## CSS and HTML Error Checking

Twine projects frequently include custom styling, HTML elements, and embedded code.

Future versions of Knot aim to provide built-in validation for CSS and HTML so mistakes can be caught while writing instead of after launching the story.

Planned improvements include:

* Detecting invalid CSS syntax
* Catching incorrect HTML structure
* Identifying missing closing tags
* Detecting invalid or unsupported styling rules
* Highlighting common mistakes directly inside the editor

This will reduce the need to rely on separate tools for debugging styles and markup.

**Status:** Planned for a future major update

---

## Smarter Context-Aware Suggestions

Different parts of a Twine passage serve different purposes.

Writing normal story text, working inside macros, embedding HTML, or writing code all follow different rules. Treating everything as plain text limits how accurate editor assistance can be.

Future improvements will allow Knot to better understand what kind of content is being written at any moment.

Planned improvements include:

* More accurate autocomplete suggestions
* Better syntax highlighting based on context
* Improved error detection
* Fewer incorrect warnings or misplaced suggestions
* Better understanding of macros and embedded code

The goal is to make the editor feel significantly more intelligent and context-aware while writing complex passages.

**Status:** Under active design

---

More features will continue to be added as Knot evolves.

The focus remains on making Twine development faster, cleaner, and easier without forcing authors to leave the editor for common tasks.

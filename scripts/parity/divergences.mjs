#!/usr/bin/env node
// Proves each entry of DIVERGENCES.md against the real upstream, for each case:
//   1. upstream's real behaviour is the bug (`upstream`),
//   2. `lq --upstream-bugs` matches upstream on every offense, of every check,
//   3. default `lq` is the fix (`fixed`),
//   4. default `lq` matches upstream on every check the divergence doesn't name.
// Fixes and suggestions are compared as the source text they produce.
//
//   node scripts/parity/divergences.mjs [--show] [DivergenceId...]
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { GEN, ORACLE, oracleRequire, runLq } from './lib.mjs';

const { createCorrector, applyFixToString } = oracleRequire('@shopify/theme-check-common');
const WORK = path.join(GEN, 'divergences');
const args = process.argv.slice(2);
const show = args.includes('--show');
const only = args.filter((a) => !a.startsWith('--'));

const BASE = {
  '.theme-check.yml': 'extends: theme-check:all\n',
  'layout/theme.liquid': '{{ content_for_header }}{{ content_for_layout }}',
};
const INDEX = { 'templates/index.json': '{"sections":{"main":{"type":"main"}},"order":["main"]}' };

// `[check, "line:col-line:col", message]` of `at`'s offenses for the named checks.
const summary = (list, checks) =>
  list
    .filter((o) => checks.includes(o.check))
    .map((o) => [o.check, `${o.start[0]}:${o.start[1]}-${o.end[0]}:${o.end[1]}`, o.message]);
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const sorted = (l) => l.map((x) => JSON.stringify(x)).sort();
// A check: `expect(list).toHave([...])`, order-independent, for those checks only.
const exactly = (checks, want) => (list) => {
  const got = sorted(summary(list, checks));
  const exp = sorted(want);
  return same(got, exp) ? null : `expected ${exp.join(' ')}\n    got      ${got.join(' ')}`;
};
const suggestions = (list, check, file) =>
  list.filter((o) => o.check === check && o.file === file).flatMap((o) => o.suggest.map((s) => s.applied));

const CASES = [
  {
    id: 'GraphNoType',
    name: 'a context override template section has no type',
    checks: ['OrphanedSnippet'],
    files: {
      ...INDEX,
      'sections/main.liquid': "{% render 'used' %}",
      'snippets/used.liquid': 'x',
      'snippets/lonely.liquid': 'y',
      'templates/index.context.b2b.json': '{"sections":{"main":{"settings":{}}},"order":["main"]}',
    },
    upstream: exactly(['OrphanedSnippet'], [
      ['OrphanedSnippet', '0:0-0:1', 'This snippet is not referenced by any other files'],
      ['OrphanedSnippet', '0:0-0:1', 'This snippet is not referenced by any other files'],
    ]),
    fixed: exactly(['OrphanedSnippet'], [['OrphanedSnippet', '0:0-0:1', 'This snippet is not referenced by any other files']]),
    extra: (r) => r.fixed.some((o) => o.file === 'snippets/lonely.liquid') || 'lonely.liquid should stay orphaned',
  },
  {
    id: 'GraphNoType',
    name: 'a template block has no type',
    checks: ['OrphanedSnippet'],
    files: {
      ...INDEX,
      'sections/main.liquid': '{% content_for "blocks" %}{% schema %}{"name":"m","blocks":[{"type":"@theme"}]}{% endschema %}',
      'blocks/b.liquid': "{% render 'used' %}{% schema %}{\"name\":\"b\"}{% endschema %}",
      'snippets/used.liquid': 'x',
      'templates/page.json': '{"sections":{"main":{"type":"main","blocks":{"a":{"settings":{}}}}},"order":["main"]}',
    },
    upstream: exactly(['OrphanedSnippet'], [['OrphanedSnippet', '0:0-0:1', 'This snippet is not referenced by any other files']]),
    fixed: exactly(['OrphanedSnippet'], []),
  },
  {
    id: 'GraphFailure',
    name: 'a template with no sections fails the graph: a snippet looks orphaned',
    checks: ['OrphanedSnippet'],
    files: { ...INDEX, 'sections/main.liquid': "{% render 'used' %}", 'snippets/used.liquid': 'x', 'templates/bad.json': '{}' },
    upstream: exactly(['OrphanedSnippet'], [['OrphanedSnippet', '0:0-0:1', 'This snippet is not referenced by any other files']]),
    fixed: exactly(['OrphanedSnippet'], []),
  },
  {
    id: 'GraphFailure',
    name: 'a failed graph: a class from a parent looks out of scope',
    checks: ['ValidScopedCSSClass', 'OrphanedSnippet'],
    files: {
      ...INDEX,
      'sections/main.liquid': "{% stylesheet %}.c { color: red }{% endstylesheet %}{% render 'used' %}",
      'snippets/used.liquid': '<i class="c"></i>',
      'templates/bad.json': '{}',
    },
    upstream: exactly(['ValidScopedCSSClass'], [['ValidScopedCSSClass', '0:10-0:11', "CSS class 'c' may be defined outside the scope of this file."]]),
    fixed: exactly(['ValidScopedCSSClass'], []),
  },
  {
    id: 'UnclosedHtmlIdentifier',
    name: 'two lookups',
    checks: ['UnclosedHTMLElement'],
    files: { 'snippets/a.liquid': '{% if a.b.c %}<a>{% endif %}' },
    upstream: exactly(['UnclosedHTMLElement'], [['UnclosedHTMLElement', '0:14-0:17', "Opening tag does not have a matching closing tag for condition `a['b'],['c']` in Document 'Document'"]]),
    fixed: exactly(['UnclosedHTMLElement'], [['UnclosedHTMLElement', '0:14-0:17', "Opening tag does not have a matching closing tag for condition `a['b']['c']` in Document 'Document'"]]),
  },
  {
    id: 'HtmlSyntaxQuotedLiteral',
    name: 'a literal holding the other quote',
    checks: ['LiquidHTMLSyntaxError'],
    files: { 'snippets/a.liquid': `{% if "it's" foo %}{% endif %}` },
    upstream: exactly(['LiquidHTMLSyntaxError'], []),
    fixed: exactly(['LiquidHTMLSyntaxError'], [['LiquidHTMLSyntaxError', '0:6-0:16', `Syntax is not supported: Expression stops at truthy value '"it's"', and will ignore: 'foo'`]]),
  },
  {
    id: 'HtmlSyntaxQuotedLiteral',
    name: 'control: without the quote, upstream reports it',
    checks: [],
    files: { 'snippets/a.liquid': `{% if "its" foo %}{% endif %}` },
    upstream: exactly(['LiquidHTMLSyntaxError'], [['LiquidHTMLSyntaxError', '0:6-0:15', `Syntax is not supported: Expression stops at truthy value '"its"', and will ignore: 'foo'`]]),
    fixed: exactly(['LiquidHTMLSyntaxError'], [['LiquidHTMLSyntaxError', '0:6-0:15', `Syntax is not supported: Expression stops at truthy value '"its"', and will ignore: 'foo'`]]),
  },
  {
    id: 'LiquidSyntaxUnclosedLine',
    name: 'unclosed block',
    checks: ['LiquidSyntaxError'],
    files: { 'snippets/a.liquid': "{% liquid\n  block 'hero'\n%}" },
    upstream: exactly(['LiquidSyntaxError'], [['LiquidSyntaxError', '0:0-2:2', "Unclosed block tag 'block' in {% liquid %} block"]]),
    fixed: exactly(['LiquidSyntaxError'], [['LiquidSyntaxError', '1:2-1:14', "Unclosed block tag 'block' in {% liquid %} block"]]),
  },
  {
    id: 'LiquidSyntaxUnclosedLine',
    name: 'the second block is the unclosed one',
    checks: ['LiquidSyntaxError'],
    files: { 'snippets/a.liquid': "{% liquid\n block 'a'\n endblock\n block 'b'\n%}" },
    upstream: exactly(['LiquidSyntaxError'], [['LiquidSyntaxError', '0:0-4:2', "Unclosed block tag 'block' in {% liquid %} block"]]),
    fixed: exactly(['LiquidSyntaxError'], [['LiquidSyntaxError', '3:1-3:10', "Unclosed block tag 'block' in {% liquid %} block"]]),
  },
  {
    id: 'LiquidSyntaxUnclosedLine',
    name: 'unclosed partial',
    checks: ['LiquidSyntaxError'],
    files: { 'snippets/a.liquid': "{% liquid\n  partial 'p'\n%}" },
    upstream: exactly(['LiquidSyntaxError'], [['LiquidSyntaxError', '0:0-2:2', "Unclosed block tag 'partial' in {% liquid %} block"]]),
    fixed: exactly(['LiquidSyntaxError'], [['LiquidSyntaxError', '1:2-1:13', "Unclosed block tag 'partial' in {% liquid %} block"]]),
  },
  {
    id: 'ContentForPrototype',
    name: '__proto__ stops the check for the file',
    checks: ['ValidContentForArguments'],
    files: { 'snippets/a.liquid': "{% content_for '__proto__' %}\n{% content_for 'block' %}" },
    upstream: exactly(['ValidContentForArguments'], []),
    fixed: exactly(['ValidContentForArguments'], [
      ['ValidContentForArguments', '1:15-1:23', `{% content_for "block" %} requires a 'id' argument`],
      ['ValidContentForArguments', '1:15-1:23', `{% content_for "block" %} requires a 'type' argument`],
    ]),
  },
  {
    id: 'AddArgumentSuggestion',
    name: 'double comma, and an empty value that is a syntax error',
    checks: ['MissingRenderSnippetArguments'],
    files: {
      'snippets/typed.liquid': '{% doc %}\n@param {object} title\n@param {string} body\n{% enddoc %}\n{{ title }}{{ body }}',
      'snippets/a.liquid': "{% render 'typed', %}",
    },
    upstream: (list) => {
      const s = suggestions(list, 'MissingRenderSnippetArguments', 'snippets/a.liquid').sort();
      return same(s, ["{% render 'typed',, body: '' %}", "{% render 'typed',, title:  %}"]) ? null : `got ${JSON.stringify(s)}`;
    },
    fixed: (list) => {
      const s = suggestions(list, 'MissingRenderSnippetArguments', 'snippets/a.liquid').sort();
      return same(s, ["{% render 'typed', body: '' %}", "{% render 'typed', title: title %}"]) ? null : `got ${JSON.stringify(s)}`;
    },
  },
  {
    id: 'AddArgumentSuggestion',
    name: 'no space before the close: upstream inserts inside the quotes',
    checks: ['MissingRenderSnippetArguments'],
    files: {
      'snippets/typed.liquid': '{% doc %}\n@param {string} body\n{% enddoc %}\n{{ body }}',
      'snippets/a.liquid': "{% render 'typed'%}",
    },
    upstream: (list) => {
      const s = suggestions(list, 'MissingRenderSnippetArguments', 'snippets/a.liquid');
      return same(s, ["{% render 'typed, body: '''%}"]) ? null : `got ${JSON.stringify(s)}`;
    },
    fixed: (list) => {
      const s = suggestions(list, 'MissingRenderSnippetArguments', 'snippets/a.liquid');
      return same(s, ["{% render 'typed', body: ''%}"]) ? null : `got ${JSON.stringify(s)}`;
    },
  },
  {
    id: 'PresetsBlockOrderPath',
    name: 'a sibling after nested blocks reports on the wrong node',
    checks: ['SchemaPresetsBlockOrder'],
    files: {
      'blocks/t.liquid': '{% schema %}{"name":"t","blocks":[{"type":"@theme"}]}{% endschema %}',
      'sections/a.liquid':
        '{% schema %}{"name":"a","blocks":[{"type":"@theme"}],"presets":[{"name":"p","blocks":{"a":{"type":"t","blocks":{"n":{"type":"t"}},"block_order":["n"]},"b":{"type":"t"}},"block_order":["a"]}]}{% endschema %}',
    },
    upstream: exactly(['SchemaPresetsBlockOrder'], [['SchemaPresetsBlockOrder', '0:144-0:149', "block 'b' is missing from the block_order"]]),
    fixed: exactly(['SchemaPresetsBlockOrder'], [['SchemaPresetsBlockOrder', '0:183-0:188', "block 'b' is missing from the block_order"]]),
  },
  {
    id: 'PresetsBlockOrderPath',
    name: 'a sibling after nested blocks throws, losing the report',
    checks: ['SchemaPresetsBlockOrder'],
    files: {
      'blocks/t.liquid': '{% schema %}{"name":"t","blocks":[{"type":"@theme"}]}{% endschema %}',
      'sections/a.liquid':
        '{% schema %}{"name":"a","blocks":[{"type":"@theme"}],"presets":[{"name":"p","blocks":{"a":{"type":"t","blocks":{"n":{"type":"t"}},"block_order":["n"]},"b":{"type":"t","blocks":{"m":{"type":"t"}}}},"block_order":["a","b"]}]}{% endschema %}',
    },
    upstream: exactly(['SchemaPresetsBlockOrder'], []),
    fixed: exactly(['SchemaPresetsBlockOrder'], [['SchemaPresetsBlockOrder', '0:194-0:195', 'block_order is missing']]),
  },
  {
    id: 'VisibleIfMatchIndex',
    name: 'text before the expression, in a section',
    checks: ['ValidVisibleIf'],
    files: {
      'sections/a.liquid': '{% schema %}{"name":"a","settings":[{"id":"x","type":"checkbox","label":"l","visible_if":"pre {{ section.settings.nope }}"}]}{% endschema %}',
    },
    upstream: exactly(['ValidVisibleIf'], [['ValidVisibleIf', '0:93-0:114', 'Invalid variable: "section.settings.nope" was not found.']]),
    fixed: exactly(['ValidVisibleIf'], [['ValidVisibleIf', '0:97-0:118', 'Invalid variable: "section.settings.nope" was not found.']]),
  },
  {
    id: 'VisibleIfMatchIndex',
    name: 'text before the expression, in settings_schema.json',
    checks: ['ValidVisibleIf'],
    files: { 'config/settings_schema.json': '[{"name":"g","settings":[{"id":"x","type":"checkbox","label":"l","visible_if":"pre {{ settings.nope }}"}]}]' },
    upstream: exactly(['ValidVisibleIf'], [['ValidVisibleIf', '0:82-0:95', 'Invalid variable: "settings.nope" was not found.']]),
    fixed: exactly(['ValidVisibleIf'], [['ValidVisibleIf', '0:86-0:99', 'Invalid variable: "settings.nope" was not found.']]),
  },
  {
    id: 'PrototypeLookup',
    name: 'validateLookup finds Object.prototype',
    checks: ['ValidVisibleIf'],
    files: { 'sections/a.liquid': '{% schema %}{"name":"a","settings":[{"id":"x","type":"checkbox","label":"l","visible_if":"{{ section.settings.toString }}"}]}{% endschema %}' },
    upstream: exactly(['ValidVisibleIf'], [['ValidVisibleIf', '0:93-0:118', 'Invalid variable: "section.settings.toString" refers to a namespace, but is being used here as a variable.']]),
    fixed: exactly(['ValidVisibleIf'], [['ValidVisibleIf', '0:93-0:118', 'Invalid variable: "section.settings.toString" was not found.']]),
  },
  {
    id: 'PrototypeLookup',
    name: 'deepGet finds Object.prototype',
    checks: ['ValidSchemaTranslations'],
    files: {
      'sections/a.liquid': '{% schema %}{"name":"a","settings":[{"id":"x","type":"checkbox","label":"t:toString"}]}{% endschema %}',
      'locales/en.default.schema.json': '{"a":"b"}',
    },
    upstream: exactly(['ValidSchemaTranslations'], []),
    fixed: exactly(['ValidSchemaTranslations'], [['ValidSchemaTranslations', '0:72-0:84', "'t:toString' does not have a matching entry in 'locales/en.default.schema.json'"]]),
  },
  {
    id: 'PrototypeLookup',
    name: 'control: own keys named length and 404 still resolve',
    checks: [],
    files: {
      'sections/a.liquid': '{% schema %}{"name":"a","settings":[{"id":"x","type":"checkbox","label":"t:settings.length"},{"id":"y","type":"checkbox","label":"t:names.404"}]}{% endschema %}',
      'locales/en.default.schema.json': '{"settings":{"length":"Length"},"names":{"404":"Not found"}}',
    },
    upstream: exactly(['ValidSchemaTranslations'], []),
    fixed: exactly(['ValidSchemaTranslations'], []),
  },
  {
    id: 'SchemaDollarCrash',
    name: 'a string $schema kills upstream with an unhandled rejection',
    checks: [],
    crashes: true,
    files: {
      'sections/a.liquid': '{% schema %}{"$schema":"https://example.test/s.json","name":"a"}{% endschema %}',
      'snippets/b.liquid': "{{ 'x' | nope }}",
    },
    fixed: exactly(['UnknownFilter'], [['UnknownFilter', '0:7-0:13', "Unknown filter 'nope' used."]]),
  },
  {
    id: 'NestedBlockPath',
    name: 'a section keyed blocks looks nested',
    checks: ['JSONMissingBlock'],
    files: {
      'sections/x.liquid': '{% schema %}{"name":"x","blocks":[{"type":"a"}]}{% endschema %}',
      'blocks/x.liquid': '{% schema %}{"name":"bx","blocks":[{"type":"b"}]}{% endschema %}',
      'blocks/a.liquid': '{% schema %}{"name":"ba"}{% endschema %}',
      'blocks/b.liquid': '{% schema %}{"name":"bb"}{% endschema %}',
      'templates/index.json': '{"sections":{"blocks":{"type":"x","blocks":{"k":{"type":"b"}}}},"order":["blocks"]}',
    },
    upstream: exactly(['JSONMissingBlock'], []),
    fixed: exactly(['JSONMissingBlock'], [['JSONMissingBlock', '0:56-0:59', "Block type 'b' is not allowed in 'sections/x.liquid'."]]),
  },
  {
    id: 'NestedBlockPath',
    name: 'control: any other key reports the same block',
    checks: [],
    files: {
      'sections/x.liquid': '{% schema %}{"name":"x","blocks":[{"type":"a"}]}{% endschema %}',
      'blocks/x.liquid': '{% schema %}{"name":"bx","blocks":[{"type":"b"}]}{% endschema %}',
      'blocks/a.liquid': '{% schema %}{"name":"ba"}{% endschema %}',
      'blocks/b.liquid': '{% schema %}{"name":"bb"}{% endschema %}',
      'templates/index.json': '{"sections":{"k":{"type":"x","blocks":{"k":{"type":"b"}}}},"order":["k"]}',
    },
    upstream: exactly(['JSONMissingBlock'], [['JSONMissingBlock', '0:51-0:54', "Block type 'b' is not allowed in 'sections/x.liquid'."]]),
    fixed: exactly(['JSONMissingBlock'], [['JSONMissingBlock', '0:51-0:54', "Block type 'b' is not allowed in 'sections/x.liquid'."]]),
  },
  {
    id: 'UnclosedTagHint',
    name: 'a missing %} is reported on the tag, not on the markup after it',
    checks: ['LiquidHTMLSyntaxError'],
    files: { 'snippets/a.liquid': '<div>\n  {% for a in b %}\n    {% render c %\n  {% endfor %}\n</div>\n' },
    upstream: exactly(['LiquidHTMLSyntaxError'], [
      ['LiquidHTMLSyntaxError', '4:0-4:6', "Attempting to close HtmlElement 'div' before it was opened"],
    ]),
    fixed: exactly(['LiquidHTMLSyntaxError'], [
      ['LiquidHTMLSyntaxError', '2:4-2:17', "This tag isn't closed, so it runs on to the `%}` on line 4. That caused: Attempting to close HtmlElement 'div' before it was opened"],
    ]),
  },
  {
    id: 'UnclosedTagHint',
    name: 'control: an opener inside a string is not an unclosed tag',
    checks: [],
    files: { 'snippets/a.liquid': '{{ "{%" }}\n</div>\n' },
    upstream: exactly(['LiquidHTMLSyntaxError'], [['LiquidHTMLSyntaxError', '1:0-1:6', "Attempting to close HtmlElement 'div' before it was opened"]]),
    fixed: exactly(['LiquidHTMLSyntaxError'], [['LiquidHTMLSyntaxError', '1:0-1:6', "Attempting to close HtmlElement 'div' before it was opened"]]),
  },
  {
    id: 'UnclosedTagHint',
    name: 'control: a stray opener in a comment is not an unclosed tag',
    checks: [],
    files: { 'snippets/a.liquid': '{% comment %} {{ {% endcomment %}\n</div>\n' },
    upstream: exactly(['LiquidHTMLSyntaxError'], [['LiquidHTMLSyntaxError', '1:0-1:6', "Attempting to close HtmlElement 'div' before it was opened"]]),
    fixed: exactly(['LiquidHTMLSyntaxError'], [['LiquidHTMLSyntaxError', '1:0-1:6', "Attempting to close HtmlElement 'div' before it was opened"]]),
  },
  {
    id: 'UnclosedTagHint',
    name: 'control: a stray opener in raw is not an unclosed tag',
    checks: [],
    files: { 'snippets/a.liquid': '{% raw %} {% {% endraw %}\n</div>\n' },
    upstream: exactly(['LiquidHTMLSyntaxError'], [['LiquidHTMLSyntaxError', '1:0-1:6', "Attempting to close HtmlElement 'div' before it was opened"]]),
    fixed: exactly(['LiquidHTMLSyntaxError'], [['LiquidHTMLSyntaxError', '1:0-1:6', "Attempting to close HtmlElement 'div' before it was opened"]]),
  },
  {
    id: 'UnclosedTagHint',
    name: 'control: an error with no unclosed tag keeps its message',
    checks: [],
    files: { 'snippets/a.liquid': 'ok\n</div>\n' },
    upstream: exactly(['LiquidHTMLSyntaxError'], [['LiquidHTMLSyntaxError', '1:0-1:6', "Attempting to close HtmlElement 'div' before it was opened"]]),
    fixed: exactly(['LiquidHTMLSyntaxError'], [['LiquidHTMLSyntaxError', '1:0-1:6', "Attempting to close HtmlElement 'div' before it was opened"]]),
  },
  {
    id: 'LinkToNoArgument',
    name: 'link_to with no argument loses later reports',
    checks: ['HardcodedRoutes'],
    files: { 'snippets/a.liquid': "{{ 'x' | link_to }}{{ 'Cart' | link_to: '/cart' }}" },
    upstream: exactly(['HardcodedRoutes'], []),
    fixed: exactly(['HardcodedRoutes'], [['HardcodedRoutes', '0:41-0:46', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart']]),
  },
  {
    id: 'EmptySchema',
    name: 'empty schema, TranslationKeyExists and PaginationSize',
    checks: ['TranslationKeyExists', 'PaginationSize'],
    files: {
      'sections/a.liquid': "{% schema %}{% endschema %}{{ 'nope' | t }}{% paginate x by 999 %}{% endpaginate %}",
      'locales/en.default.json': '{"a":"b"}',
    },
    upstream: exactly(['TranslationKeyExists', 'PaginationSize'], []),
    fixed: exactly(['TranslationKeyExists', 'PaginationSize'], [
      ['TranslationKeyExists', '0:30-0:36', "'nope' does not have a matching entry in 'locales/en.default.json'"],
      ['PaginationSize', '0:60-0:63', 'Pagination size must be a positive integer between 1 and 250.'],
    ]),
  },
  {
    id: 'EmptySchema',
    name: 'null schema, TranslationKeyExists',
    checks: ['TranslationKeyExists'],
    files: {
      'sections/a.liquid': "{% schema %}null{% endschema %}{{ 'nope' | t }}",
      'locales/en.default.json': '{"a":"b"}',
    },
    upstream: exactly(['TranslationKeyExists'], []),
    fixed: exactly(['TranslationKeyExists'], [['TranslationKeyExists', '0:34-0:40', "'nope' does not have a matching entry in 'locales/en.default.json'"]]),
  },
  {
    id: 'EmptySchema',
    name: 'empty schema, AssetSizeAppBlockCSS and AssetSizeAppBlockJavaScript',
    checks: ['AssetSizeAppBlockCSS', 'AssetSizeAppBlockJavaScript'],
    files: {
      '.theme-check.yml': 'extends: theme-check:all\nAssetSizeAppBlockCSS:\n  enabled: true\nAssetSizeAppBlockJavaScript:\n  enabled: true\n',
      'blocks/a.liquid': '{% schema %}{% endschema %}{% schema %}{"stylesheet":"nope.css","javascript":"nope.js"}{% endschema %}',
    },
    upstream: exactly(['AssetSizeAppBlockCSS', 'AssetSizeAppBlockJavaScript'], []),
    fixed: exactly(['AssetSizeAppBlockCSS', 'AssetSizeAppBlockJavaScript'], [
      ['AssetSizeAppBlockCSS', '0:54-0:62', "'nope.css' does not exist."],
      ['AssetSizeAppBlockJavaScript', '0:78-0:85', "'nope.js' does not exist."],
    ]),
  },
  {
    id: 'NullInPath',
    name: 'a null along a translation key',
    checks: ['TranslationKeyExists'],
    files: { 'snippets/a.liquid': "{{ 'a.c' | t }}{{ 'y' | t }}", 'locales/en.default.json': '{"a":null}' },
    upstream: exactly(['TranslationKeyExists'], []),
    fixed: exactly(['TranslationKeyExists'], [
      ['TranslationKeyExists', '0:3-0:8', "'a.c' does not have a matching entry in 'locales/en.default.json'"],
      ['TranslationKeyExists', '0:18-0:21', "'y' does not have a matching entry in 'locales/en.default.json'"],
    ]),
  },
  {
    id: 'NullInPath',
    name: 'a null settings item',
    checks: ['PaginationSize'],
    files: { 'sections/a.liquid': '{% paginate x by section.settings.s %}{% endpaginate %}{% schema %}{"name":"a","settings":[null,{"id":"s","type":"number","label":"l","default":999}]}{% endschema %}' },
    upstream: exactly(['PaginationSize'], []),
    fixed: exactly(['PaginationSize'], [['PaginationSize', '0:17-0:35', "This setting's default value should be between 1 and 250 but is currently 999."]]),
  },
  {
    id: 'EmptyAssetString',
    name: 'an empty string before stylesheet_tag',
    checks: ['AssetSizeCSS'],
    files: {
      '.theme-check.yml': 'extends: theme-check:all\nAssetSizeCSS:\n  enabled: true\n',
      'snippets/a.liquid': "{{ '' | stylesheet_tag }}{{ 'big.css' | asset_url | stylesheet_tag }}",
      'assets/big.css': 'x'.repeat(200000),
    },
    upstream: exactly(['AssetSizeCSS'], []),
    fixed: exactly(['AssetSizeCSS'], [['AssetSizeCSS', '0:28-0:67', 'The CSS file size exceeds the threshold of 100000 bytes']]),
  },
  {
    id: 'EmptyAssetString',
    name: 'an empty string before script_tag',
    checks: ['AssetSizeJavaScript'],
    files: {
      '.theme-check.yml': 'extends: theme-check:all\nAssetSizeJavaScript:\n  enabled: true\n',
      'snippets/a.liquid': "{{ '' | script_tag }}{{ 'big.js' | asset_url | script_tag }}",
      'assets/big.js': 'x'.repeat(200000),
    },
    upstream: exactly(['AssetSizeJavaScript'], []),
    fixed: exactly(['AssetSizeJavaScript'], [['AssetSizeJavaScript', '0:24-0:58', 'JavaScript on every page load exceeds compressed size threshold (10000 Bytes), consider using the import on interaction pattern.']]),
  },
  {
    id: 'CdnPreconnectHost',
    name: 'missed without a path, wrong on lookalike hosts',
    checks: ['CdnPreconnect'],
    files: {
      'snippets/a.liquid':
        '<link rel="preconnect" href="https://cdn.shopify.com">\n<link rel="preconnect" href="https://xcdnXshopify.comy/a">\n<link rel="preconnect" href="https://cdn.shopify.com/">\n<link rel="preconnect" href="https://acdn.shopify.com.evil/">\n<link rel="preconnect" href="//CDN.shopify.com/x">',
    },
    upstream: exactly(['CdnPreconnect'], [
      ['CdnPreconnect', '1:0-1:58', 'Preconnecting to cdn.shopify.com is unnecessary and can lead to worse performance'],
      ['CdnPreconnect', '2:0-2:55', 'Preconnecting to cdn.shopify.com is unnecessary and can lead to worse performance'],
      ['CdnPreconnect', '3:0-3:61', 'Preconnecting to cdn.shopify.com is unnecessary and can lead to worse performance'],
    ]),
    fixed: exactly(['CdnPreconnect'], [
      ['CdnPreconnect', '0:0-0:54', 'Preconnecting to cdn.shopify.com is unnecessary and can lead to worse performance'],
      ['CdnPreconnect', '2:0-2:55', 'Preconnecting to cdn.shopify.com is unnecessary and can lead to worse performance'],
      ['CdnPreconnect', '4:0-4:50', 'Preconnecting to cdn.shopify.com is unnecessary and can lead to worse performance'],
    ]),
  },
  {
    id: 'RouteBoundary',
    name: 'routes match by prefix',
    checks: ['HardcodedRoutes'],
    files: {
      'snippets/a.liquid':
        `<a href="/collections/allx">a</a>\n<a href="/cartoons">b</a>\n<a href="/cart?x=1">c</a>\n<a href="/cart#a">c</a>\n<a href="/cart/">c</a>\n<a href="/cart">c</a>\n{{ 'x' | link_to: '/cartoons' }}\n{{ 'x' | link_to: '/cart' }}`,
    },
    upstream: exactly(['HardcodedRoutes'], [
      ['HardcodedRoutes', '0:9-0:25', 'Use routes object {{ routes.all_products_collection_url }} instead of hardcoding /collections/all'],
      ['HardcodedRoutes', '1:9-1:14', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart'],
      ['HardcodedRoutes', '2:9-2:14', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart'],
      ['HardcodedRoutes', '3:9-3:14', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart'],
      ['HardcodedRoutes', '4:9-4:14', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart'],
      ['HardcodedRoutes', '5:9-5:14', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart'],
      ['HardcodedRoutes', '6:19-6:24', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart'],
      ['HardcodedRoutes', '7:19-7:24', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart'],
    ]),
    fixed: exactly(['HardcodedRoutes'], [
      ['HardcodedRoutes', '0:9-0:21', 'Use routes object {{ routes.collections_url }} instead of hardcoding /collections'],
      ['HardcodedRoutes', '2:9-2:14', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart'],
      ['HardcodedRoutes', '3:9-3:14', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart'],
      ['HardcodedRoutes', '4:9-4:14', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart'],
      ['HardcodedRoutes', '5:9-5:14', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart'],
      ['HardcodedRoutes', '7:19-7:24', 'Use routes object {{ routes.cart_url }} instead of hardcoding /cart'],
    ]),
  },
  {
    id: 'ImgUrlPrototype',
    name: 'product_img_url: the suggestion prints a native function',
    checks: ['DeprecatedFilter'],
    files: { 'snippets/a.liquid': "{{ p | product_img_url: 'toString' }}" },
    upstream: (list) => {
      const s = suggestions(list, 'DeprecatedFilter', 'snippets/a.liquid');
      return same(s, ['{{ p | image_url: width: function toString() { [native code] }, height: function toString() { [native code] } }}']) ? null : `got ${JSON.stringify(s)}`;
    },
    fixed: (list) => {
      const s = suggestions(list, 'DeprecatedFilter', 'snippets/a.liquid');
      return same(s, ['{{ p | image_url: width: 100 }}']) ? null : `got ${JSON.stringify(s)}`;
    },
  },
  {
    id: 'ImgUrlPrototype',
    name: 'img_tag: the suggestion prints a native function',
    checks: ['DeprecatedFilter'],
    files: { 'snippets/a.liquid': "{{ p | img_tag: 'a', 'b', 'toString' }}" },
    upstream: (list) => {
      const s = suggestions(list, 'DeprecatedFilter', 'snippets/a.liquid');
      return /native code/.test(s[0]) ? null : `got ${JSON.stringify(s)}`;
    },
    fixed: (list) => {
      const s = suggestions(list, 'DeprecatedFilter', 'snippets/a.liquid');
      return !/native code/.test(s[0]) ? null : `got ${JSON.stringify(s)}`;
    },
  },
  {
    id: 'ImgUrlPrototype',
    name: 'control: img_url scales the size first, so upstream prints no function there',
    checks: [],
    files: { 'snippets/a.liquid': "{{ p | img_url: 'toString' }}" },
    upstream: (list) => (same(suggestions(list, 'DeprecatedFilter', 'snippets/a.liquid'), ['{{ p | image_url: width: 100 }}']) ? null : 'differs'),
    fixed: (list) => (same(suggestions(list, 'DeprecatedFilter', 'snippets/a.liquid'), ['{{ p | image_url: width: 100 }}']) ? null : 'differs'),
  },
];

// Runs in a child, because some of upstream's failures take the process down.
const ORACLE_RUNNER = `
const { themeCheckRun } = require(${JSON.stringify(path.join(ORACLE, 'node_modules/@shopify/theme-check-node'))});
const { createCorrector, applyFixToString } = require(${JSON.stringify(path.join(ORACLE, 'node_modules/@shopify/theme-check-common'))});
const fs = require('node:fs'), path = require('node:path');
const root = process.argv[1];
console.error = () => {};
const apply = (o, fix) => {
  const source = fs.readFileSync(o.uri.replace(/^file:\\/\\//, ''), 'utf8');
  const corrector = createCorrector(o.type, source);
  fix(corrector);
  try { return applyFixToString(source, corrector.fix); } catch { return null; }
};
themeCheckRun(root, undefined, () => {}).then(({ offenses }) => {
  const list = offenses.map((o) => ({
    file: path.relative(root, o.uri.replace(/^file:\\/\\//, '')),
    check: o.check,
    message: o.message,
    start: [o.start.line, o.start.character],
    end: [o.end.line, o.end.character],
    fixed: o.fix ? apply(o, o.fix) : null,
    suggest: (o.suggest ?? []).map((s) => ({ message: s.message, applied: apply(o, s.fix) })),
  }));
  process.stdout.write(JSON.stringify(list));
});
`;

function runOracle(root) {
  const r = spawnSync('node', ['-e', ORACLE_RUNNER, root], { encoding: 'utf8', maxBuffer: 1 << 28 });
  return { status: r.status, stderr: r.stderr, list: r.status === 0 ? JSON.parse(r.stdout) : null };
}

const lqRun = (root, flags) => JSON.parse(runLq(['debug-offenses', '--path', root, ...flags]));

const offenseKey = (o) => JSON.stringify([o.file, o.check, o.message, o.start, o.end, o.fixed, o.suggest]);
function diff(a, b) {
  const count = (l) => l.reduce((m, o) => m.set(offenseKey(o), (m.get(offenseKey(o)) ?? 0) + 1), new Map());
  const ca = count(a), cb = count(b);
  return { missing: [...ca].filter(([k, n]) => (cb.get(k) ?? 0) < n).map(([k]) => k), extra: [...cb].filter(([k, n]) => (ca.get(k) ?? 0) < n).map(([k]) => k) };
}

const known = runLq(['debug-divergences']).trim().split('\n').map((l) => l.split('\t')[0]);
// Divergences that write to disk are proven by their own harness, which must pass.
const EXTERNAL = { AutofixAbort: 'autofix.mjs' };
const covered = new Set([...CASES.map((c) => c.id), ...Object.keys(EXTERNAL)]);
const uncovered = known.filter((id) => !covered.has(id));
const unknown = [...covered].filter((id) => !known.includes(id));

let failed = 0;
const perId = new Map();
fs.rmSync(WORK, { recursive: true, force: true });
CASES.forEach((c, i) => {
  if (only.length && !only.includes(c.id)) return;
  const root = path.join(WORK, `${i}-${c.id}`);
  for (const [f, content] of Object.entries({ ...BASE, ...c.files })) {
    const p = path.join(root, f);
    fs.mkdirSync(path.dirname(p), { recursive: true });
    fs.writeFileSync(p, content);
  }
  const problems = [];
  const oracle = runOracle(root);
  const bugs = lqRun(root, ['--upstream-bugs']);
  const fixed = lqRun(root, []);

  if (c.crashes) {
    if (oracle.status === 0) problems.push('upstream did not crash');
    else if (!/\$schema/.test(oracle.stderr)) problems.push(`upstream crashed differently: ${oracle.stderr.slice(0, 200)}`);
    if (JSON.stringify(bugs) !== JSON.stringify(fixed)) problems.push('--upstream-bugs should continue as the default does');
  } else {
    if (oracle.status !== 0) problems.push(`upstream crashed: ${oracle.stderr.slice(0, 300)}`);
    else {
      const upstreamProblem = c.upstream(oracle.list);
      if (upstreamProblem) problems.push(`upstream isn't the reported behaviour: ${upstreamProblem}`);
      const d = diff(oracle.list, bugs);
      if (d.missing.length || d.extra.length) {
        problems.push(`--upstream-bugs differs from upstream:\n    missing ${d.missing.join('\n    missing ')}\n    extra ${d.extra.join('\n    extra ')}`);
      }
      // Everything outside the named checks stays upstream's in default mode.
      const outside = (l) => l.filter((o) => !c.checks.includes(o.check));
      const e = diff(outside(oracle.list), outside(fixed));
      if (e.missing.length || e.extra.length) {
        problems.push(`default mode changes other checks:\n    missing ${e.missing.join('\n    missing ')}\n    extra ${e.extra.join('\n    extra ')}`);
      }
    }
  }
  const fixedProblem = c.fixed(fixed);
  if (fixedProblem) problems.push(`default isn't the fix: ${fixedProblem}`);
  if (c.extra) {
    const r = c.extra({ fixed, bugs, oracle: oracle.list });
    if (r !== true) problems.push(String(r));
  }
  const status = problems.length ? 'FAIL' : 'ok';
  if (problems.length) failed++;
  perId.set(c.id, (perId.get(c.id) ?? true) && !problems.length);
  console.log(`[divergence/${c.id}] ${status}: ${c.name}`);
  for (const p of problems) console.log(`  ${p}`);
  if (show) {
    console.log(`  upstream: ${JSON.stringify(oracle.list && summary(oracle.list, c.checks))}`);
    console.log(`  default:  ${JSON.stringify(summary(fixed, c.checks))}`);
  }
});
if (!only.length) {
  if (uncovered.length) { console.log(`[divergence] no case proves: ${uncovered.join(' ')}`); failed++; }
  if (unknown.length) { console.log(`[divergence] cases for unknown ids: ${unknown.join(' ')}`); failed++; }
}
for (const [id, script] of Object.entries(EXTERNAL)) {
  if (only.length && !only.includes(id)) continue;
  const run = spawnSync(process.execPath, [path.join(path.dirname(fileURLToPath(import.meta.url)), script)], { encoding: 'utf8' });
  perId.set(id, run.status === 0);
  if (run.status !== 0) failed++;
  console.log(`[divergence/${id}] ${run.status === 0 ? 'ok' : 'FAIL'}: ${script}`);
}
const ok = [...perId.values()].filter(Boolean).length;
console.log(`[divergence] ${ok}/${perId.size} divergences proven, ${failed} failures`);
process.exit(failed ? 1 : 0);

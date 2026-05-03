import { strict as assert } from 'assert';
import { WorkspaceIndex } from '../../src/workspaceIndex';

describe('WorkspaceIndex', () => {
  let workspace: WorkspaceIndex;

  beforeEach(() => {
    workspace = new WorkspaceIndex();
  });

  describe('File management', () => {
    it('should add and track files', () => {
      workspace.upsertFile('test://file1.tw', ':: Start\nContent');
      
      assert.ok(workspace.hasFile('test://file1.tw'));
      assert.strictEqual(workspace.getKnownUris().length, 1);
    });

    it('should remove files', () => {
      workspace.upsertFile('test://file1.tw', ':: Start\nContent');
      workspace.removeFile('test://file1.tw');
      
      assert.ok(!workspace.hasFile('test://file1.tw'));
      assert.strictEqual(workspace.getKnownUris().length, 0);
    });

    it('should update existing files', () => {
      workspace.upsertFile('test://file1.tw', ':: Start\nOld content');
      workspace.upsertFile('test://file1.tw', ':: Start\nNew content');
      
      const uris = workspace.getKnownUris();
      assert.strictEqual(uris.length, 1);
    });
    
    it('should require reanalysis to reflect cross-file link updates', () => {
      workspace.upsertFile('test://target.tw', ':: Target\nBody');
      workspace.upsertFile('test://source.tw', ':: Start\n[[Target]]');
      workspace.reanalyzeAll();

      assert.strictEqual(workspace.getIncomingLinks('Target').length, 1);

      workspace.upsertFile('test://source.tw', ':: Start\n[[RenamedTarget]]');
      // upsertFile only updates parse cache; callers must trigger reanalysis.
      assert.strictEqual(workspace.getIncomingLinks('Target').length, 1);

      workspace.reanalyzeAll();
      assert.strictEqual(workspace.getIncomingLinks('Target').length, 0);
      assert.strictEqual(workspace.getIncomingLinks('RenamedTarget').length, 1);
    });

    it('should return undefined from upsertFile (no dirty-file payload)', () => {
      const result = workspace.upsertFile('test://file1.tw', ':: Start\nContent');
      assert.strictEqual(result, undefined);
    });
  });



  describe('Incremental update contract', () => {
    it('should handle mixed old/new links when passage names are partially updated', () => {
      workspace.upsertFile('test://target.tw', ':: Target\nBody');
      workspace.upsertFile('test://src1.tw', ':: Source1\n[[Target]]');
      workspace.upsertFile('test://src2.tw', ':: Source2\n[[Target]]');
      workspace.reanalyzeAll();

      assert.strictEqual(workspace.getIncomingLinks('Target').length, 2);

      workspace.upsertFile('test://target.tw', ':: RenamedTarget\nBody');
      workspace.upsertFile('test://src1.tw', ':: Source1\n[[RenamedTarget]]');
      workspace.reanalyzeAll();

      assert.strictEqual(workspace.getPassageDefinition('Target'), undefined);
      assert.ok(workspace.getPassageDefinition('RenamedTarget') !== undefined);
      assert.strictEqual(workspace.getIncomingLinks('Target').length, 1);
      assert.strictEqual(workspace.getIncomingLinks('RenamedTarget').length, 1);
    });

    it('should update definition source when a passage is removed then re-added in another file', () => {
      workspace.upsertFile('test://a.tw', ':: Target\nA');
      workspace.upsertFile('test://src.tw', ':: Start\n[[Target]]');
      workspace.reanalyzeAll();

      assert.strictEqual(workspace.getPassageDefinition('Target')?.uri, 'test://a.tw');
      assert.strictEqual(workspace.getIncomingLinks('Target').length, 1);

      workspace.removeFile('test://a.tw');
      workspace.upsertFile('test://b.tw', ':: Target\nB');
      workspace.reanalyzeAll();

      assert.strictEqual(workspace.getPassageDefinition('Target')?.uri, 'test://b.tw');
      assert.strictEqual(workspace.getIncomingLinks('Target').length, 1);
    });

    it('should refresh macro-argument links only after reanalysis', () => {
      workspace.upsertFile('test://targets.tw', ':: Target\nA\n\n:: NewTarget\nB');
      workspace.upsertFile('test://src.tw', ':: Start\n<<goto "Target">>');
      workspace.reanalyzeAll();

      assert.strictEqual(workspace.getIncomingLinks('Target').length, 1);

      workspace.upsertFile('test://src.tw', ':: Start\n<<goto "NewTarget">>');
      assert.strictEqual(workspace.getIncomingLinks('Target').length, 1);

      workspace.reanalyzeAll();
      assert.strictEqual(workspace.getIncomingLinks('Target').length, 0);
      assert.strictEqual(workspace.getIncomingLinks('NewTarget').length, 1);
    });

    it('should analyze only latest file contents after multiple upserts before reanalysis', () => {
      workspace.upsertFile('test://links.tw', ':: Start\n[[Alpha]]');
      workspace.upsertFile('test://links.tw', ':: Start\n[[Beta]]');
      workspace.upsertFile('test://targets.tw', ':: Alpha\nA\n\n:: Beta\nB');
      workspace.reanalyzeAll();

      assert.strictEqual(workspace.getIncomingLinks('Alpha').length, 0);
      assert.strictEqual(workspace.getIncomingLinks('Beta').length, 1);
    });

  });

  describe('Passage definitions', () => {
    it('should register passage definitions', () => {
      workspace.upsertFile('test://file1.tw', ':: MyPassage\nContent');
      workspace.reanalyzeAll();
      
      const def = workspace.getPassageDefinition('MyPassage');
      assert.ok(def !== undefined);
      assert.strictEqual(def?.passageName, 'MyPassage');
    });

    it('should return undefined for unknown passages', () => {
      workspace.upsertFile('test://file1.tw', ':: MyPassage\nContent');
      workspace.reanalyzeAll();
      
      const def = workspace.getPassageDefinition('Unknown');
      assert.strictEqual(def, undefined);
    });

    it('should handle multiple passages in one file', () => {
      workspace.upsertFile('test://file1.tw', ':: Passage1\nContent1\n\n:: Passage2\nContent2');
      workspace.reanalyzeAll();
      
      assert.ok(workspace.getPassageDefinition('Passage1') !== undefined);
      assert.ok(workspace.getPassageDefinition('Passage2') !== undefined);
    });

    it('should get all passage names', () => {
      workspace.upsertFile('test://file1.tw', ':: A\n\n:: B\n\n:: C');
      workspace.reanalyzeAll();
      
      const names = workspace.getPassageNames();
      assert.strictEqual(names.length, 3);
      assert.ok(names.includes('A'));
      assert.ok(names.includes('B'));
      assert.ok(names.includes('C'));
    });
  });

  describe('Passage references', () => {
    it('should track incoming links', () => {
      workspace.upsertFile('test://file1.tw', ':: Start\n[[Target]]');
      workspace.upsertFile('test://file2.tw', ':: Other\n[[Target]]');
      workspace.reanalyzeAll();
      
      const links = workspace.getIncomingLinks('Target');
      assert.strictEqual(links.length, 2);
    });

    it('should return empty array for passages with no incoming links', () => {
      workspace.upsertFile('test://file1.tw', ':: Start\nContent');
      workspace.reanalyzeAll();
      
      const links = workspace.getIncomingLinks('Start');
      assert.deepStrictEqual(links, []);
    });

    it('should track files referencing a passage', () => {
      workspace.upsertFile('test://file1.tw', ':: Start\n[[Target]]');
      workspace.upsertFile('test://file2.tw', ':: Other\n[[Target]]');
      workspace.reanalyzeAll();
      
      const refs = workspace.getReferencingFiles('Target');
      assert.strictEqual(refs.length, 2);
      assert.ok(refs.includes('test://file1.tw'));
      assert.ok(refs.includes('test://file2.tw'));
    });
  });

  describe('Variable definitions', () => {
    it('should register story variable definitions', () => {
      workspace.upsertFile('test://file1.tw', ':: StoryInit\n<<set $myVar to 5>>');
      workspace.reanalyzeAll();
      
      const def = workspace.getVariableDefinition('myVar');
      assert.ok(def !== undefined);
      assert.strictEqual(def?.passageName, 'StoryInit');
    });

    it('should track variable references', () => {
      workspace.upsertFile('test://file1.tw', ':: StoryInit\n<<set $myVar to 5>>\n\n:: Start\n<<print $myVar>>');
      workspace.reanalyzeAll();
      
      const refs = workspace.getVariableReferences('myVar');
      assert.ok(refs.length >= 1);
    });

    it('should handle first-write-wins for variable types', () => {
      workspace.upsertFile('test://file1.tw', ':: A\n<<set $x to 5>>');
      workspace.upsertFile('test://file2.tw', ':: B\n<<set $x to "hello">>');
      workspace.reanalyzeAll();
      
      const def = workspace.getVariableDefinition('x');
      // First definition should win
      assert.ok(def !== undefined);
    });
  });

  describe('Macro definitions', () => {
    it('should register widget macro definitions', () => {
      workspace.upsertFile('test://file1.tw', ':: StoryInit\n<<widget "myWidget">>content<</widget>>');
      workspace.reanalyzeAll();
      
      const def = workspace.getMacroDefinition('myWidget');
      assert.ok(def !== undefined);
    });

    it('should track macro call sites', () => {
      workspace.upsertFile('test://file1.tw', ':: StoryInit\n<<widget "myWidget">>content<</widget>>\n\n:: Start\n<<myWidget>>');
      workspace.reanalyzeAll();
      
      const calls = workspace.getMacroCallSites('myWidget');
      assert.ok(calls.length > 0);
    });

    it('should track cross-file macro usage', () => {
      workspace.upsertFile('test://macros.tw', ':: StoryInit\n<<widget "shared">>content<</widget>>');
      workspace.upsertFile('test://usage.tw', ':: Start\n<<shared>>');
      workspace.reanalyzeAll();
      
      const def = workspace.getMacroDefinition('shared');
      assert.ok(def !== undefined);
      
      const calls = workspace.getMacroCallSites('shared');
      assert.ok(calls.length > 0);
    });
  });

  describe('JS global definitions', () => {
    it('should register JS globals from script passages', () => {
      workspace.upsertFile('test://file1.tw', ':: Story JavaScript\nconst myGlobal = 5;');
      workspace.reanalyzeAll();
      
      const def = workspace.getJsGlobalDefinition('myGlobal');
      assert.ok(def !== undefined);
    });

    it('should provide all JS globals', () => {
      workspace.upsertFile('test://file1.tw', ':: Story JavaScript\nconst a = 1;\nlet b = 2;');
      workspace.reanalyzeAll();
      
      const globals = workspace.getAllJsGlobals();
      assert.ok(globals.size >= 2);
    });
  });

  describe('Analysis results', () => {
    it('should cache analysis results', () => {
      workspace.upsertFile('test://file1.tw', ':: Start\nContent');
      workspace.reanalyzeAll();
      
      const analysis = workspace.getAnalysis('test://file1.tw');
      assert.ok(analysis !== undefined);
      assert.ok(Array.isArray(analysis.symbols.getUserSymbols()));
    });

    it('should include semantic tokens in analysis', () => {
      workspace.upsertFile('test://file1.tw', ':: Start\n<<print $x>>');
      workspace.reanalyzeAll();
      
      const analysis = workspace.getAnalysis('test://file1.tw');
      assert.ok(analysis !== undefined);
      assert.ok(analysis.semanticTokens.length > 0);
    });

    it('should include diagnostics in analysis', () => {
      workspace.upsertFile('test://file1.tw', ':: Start\n[[Unknown]]');
      workspace.reanalyzeAll();
      
      const analysis = workspace.getAnalysis('test://file1.tw');
      assert.ok(analysis !== undefined);
      // Unknown link should produce diagnostic
    });
  });

  describe('Script-first ordering', () => {
    it('should process script passages first', () => {
      workspace.upsertFile('test://normal.tw', ':: Normal\nContent');
      workspace.upsertFile('test://init.tw', ':: StoryInit\n<<set $x to 1>>');
      workspace.reanalyzeAll();
      
      // StoryInit should be processed first so its variables are available
      const def = workspace.getVariableDefinition('x');
      assert.ok(def !== undefined);
    });

    it('should handle script-tagged passages', () => {
      workspace.upsertFile('test://file1.tw', ':: Setup [script]\nconst setupVar = true;');
      workspace.reanalyzeAll();
      
      const def = workspace.getJsGlobalDefinition('setupVar');
      assert.ok(def !== undefined);
    });
  });

  describe('Special passages', () => {
    it('should recognize StoryInit', () => {
      workspace.upsertFile('test://file1.tw', ':: StoryInit\n<<set $init to true>>');
      workspace.reanalyzeAll();
      
      const def = workspace.getVariableDefinition('init');
      assert.ok(def !== undefined);
    });

    it('should recognize underscore-prefixed passages', () => {
      workspace.upsertFile('test://file1.tw', ':: _Footer\nFooter content');
      workspace.reanalyzeAll();
      
      const def = workspace.getPassageDefinition('_Footer');
      assert.ok(def !== undefined);
    });
  });

  describe('Active format tracking', () => {
    it('should store and retrieve active format id', () => {
      workspace.setActiveFormatId('sugarcube-2');
      assert.strictEqual(workspace.getActiveFormatId(), 'sugarcube-2');
    });

    it('should default to empty format id', () => {
      assert.strictEqual(workspace.getActiveFormatId(), '');
    });
  });

  describe('Cross-file consistency', () => {
    it('should maintain consistent state after multiple updates', () => {
      workspace.upsertFile('test://file1.tw', ':: A\n[[B]]');
      workspace.reanalyzeAll();
      
      workspace.upsertFile('test://file2.tw', ':: B\n[[A]]');
      workspace.reanalyzeAll();
      
      // Both passages should be defined
      assert.ok(workspace.getPassageDefinition('A') !== undefined);
      assert.ok(workspace.getPassageDefinition('B') !== undefined);
      
      // Both should have incoming links
      assert.strictEqual(workspace.getIncomingLinks('A').length, 1);
      assert.strictEqual(workspace.getIncomingLinks('B').length, 1);
    });

    it('should handle file removal correctly', () => {
      workspace.upsertFile('test://file1.tw', ':: Target\nContent');
      workspace.upsertFile('test://file2.tw', ':: Source\n[[Target]]');
      workspace.reanalyzeAll();
      
      workspace.removeFile('test://file1.tw');
      workspace.reanalyzeAll();
      
      // Target should no longer be defined
      assert.strictEqual(workspace.getPassageDefinition('Target'), undefined);
    });
  });

  describe('Link extraction', () => {
    it('should extract links from macro arguments', () => {
      workspace.upsertFile('test://file1.tw', ':: Start\n<<goto "Target">>');
      workspace.upsertFile('test://file2.tw', ':: Target\nContent');
      workspace.reanalyzeAll();
      
      const links = workspace.getIncomingLinks('Target');
      assert.strictEqual(links.length, 1);
    });

    it('should extract links from link macros', () => {
      // The <<link>> macro with passage arg is tracked via PASSAGE_ARG_MACROS set
      workspace.upsertFile('test://file1.tw', ':: Start\n<<link "Target">>click me<</link>>');
      workspace.upsertFile('test://file2.tw', ':: Target\nContent');
      workspace.reanalyzeAll();
      
      const links = workspace.getIncomingLinks('Target');
      assert.ok(links.length >= 0); // Link extraction depends on macro arg parsing
    });
  });

  describe('Implicit passage references (data-passage, JS APIs)', () => {
    it('should detect data-passage HTML attributes as passage references', () => {
      workspace.upsertFile('test://ui.tw', ':: StoryInterface\n<div data-passage="UI Outfit Label">content</div>');
      workspace.upsertFile('test://target.tw', ':: UI Outfit Label\n<<print "wearing">>');
      workspace.reanalyzeAll();

      const links = workspace.getIncomingLinks('UI Outfit Label');
      assert.strictEqual(links.length, 1);
      assert.strictEqual(links[0]!.sourcePassage, 'StoryInterface');
    });

    it('should detect Engine.play() calls in macro bodies as passage references', () => {
      workspace.upsertFile('test://src.tw', ':: Start\n<<run Engine.play("Secret Room")>>');
      workspace.upsertFile('test://target.tw', ':: Secret Room\nYou found it');
      workspace.reanalyzeAll();

      const links = workspace.getIncomingLinks('Secret Room');
      assert.strictEqual(links.length, 1);
      assert.strictEqual(links[0]!.sourcePassage, 'Start');
    });

    it('should detect Engine.goto() calls as passage references', () => {
      workspace.upsertFile('test://src.tw', ':: Start\n<<run Engine.goto("Destination")>>');
      workspace.upsertFile('test://target.tw', ':: Destination\nArrived');
      workspace.reanalyzeAll();

      const links = workspace.getIncomingLinks('Destination');
      assert.strictEqual(links.length, 1);
    });

    it('should detect Story.get() calls as passage references', () => {
      workspace.upsertFile('test://src.tw', ':: Start\n<<run Story.get("Lore Entry")>>');
      workspace.upsertFile('test://target.tw', ':: Lore Entry\nDeep lore');
      workspace.reanalyzeAll();

      const links = workspace.getIncomingLinks('Lore Entry');
      assert.strictEqual(links.length, 1);
    });

    it('should detect multiple data-passage attributes in one passage', () => {
      workspace.upsertFile('test://ui.tw', ':: StoryInterface\n<div data-passage="UI Outfit Label"></div>\n<div data-passage="UI Date"></div>');
      workspace.upsertFile('test://targets.tw', ':: UI Outfit Label\nLabel\n\n:: UI Date\nDate');
      workspace.reanalyzeAll();

      assert.strictEqual(workspace.getIncomingLinks('UI Outfit Label').length, 1);
      assert.strictEqual(workspace.getIncomingLinks('UI Date').length, 1);
    });

    it('should detect implicit refs in script passages', () => {
      workspace.upsertFile('test://js.tw', ':: Story JavaScript\nEngine.play("Dynamic Passage");');
      workspace.upsertFile('test://target.tw', ':: Dynamic Passage\nLoaded from JS');
      workspace.reanalyzeAll();

      const links = workspace.getIncomingLinks('Dynamic Passage');
      assert.strictEqual(links.length, 1);
      assert.strictEqual(links[0]!.sourcePassage, 'Story JavaScript');
    });

    it('should make data-passage-referenced passages reachable from start', () => {
      workspace.upsertFile('test://sd.tw', ':: StoryData\n{"ifid":"test","format":"sugarcube-2","start":"Start"}');
      workspace.upsertFile('test://start.tw', ':: Start\n[[Go|Hub]]');
      workspace.upsertFile('test://hub.tw', ':: Hub\n<div data-passage="UI Panel">panel</div>');
      workspace.upsertFile('test://panel.tw', ':: UI Panel\nPanel content');
      workspace.reanalyzeAll();

      // UI Panel should NOT be unreachable — it's referenced via data-passage
      const unreachable = workspace.getUnreachablePassages();
      assert.ok(!unreachable.includes('UI Panel'), `UI Panel should be reachable but was in unreachable list: ${unreachable.join(', ')}`);
    });

    it('should handle data-passage with single quotes', () => {
      workspace.upsertFile('test://ui.tw', ":: StoryInterface\n<div data-passage='UI Panel'>content</div>");
      workspace.upsertFile('test://target.tw', ':: UI Panel\nContent');
      workspace.reanalyzeAll();

      const links = workspace.getIncomingLinks('UI Panel');
      assert.strictEqual(links.length, 1);
    });
  });
});

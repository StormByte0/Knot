export type SourceRange = { start: number; end: number };

export enum TokenType {
  PassageMarker = 'PassageMarker',
  PassageName = 'PassageName',
  PassageTag = 'PassageTag',
  PassageMetadata = 'PassageMetadata',

  MacroOpen = 'MacroOpen',
  MacroName = 'MacroName',
  MacroClose = 'MacroClose',
  MacroCloseOpen = 'MacroCloseOpen',

  LinkOpen = 'LinkOpen',
  LinkClose = 'LinkClose',
  LinkSeparator = 'LinkSeparator',
  HtmlOpen = 'HtmlOpen',
  HtmlClose = 'HtmlClose',
  HtmlCloseTag = 'HtmlCloseTag',
  HtmlAttribute = 'HtmlAttribute',

  StoryVar = 'StoryVar',
  TempVar = 'TempVar',
  Identifier = 'Identifier',
  Number = 'Number',
  String = 'String',
  Regex = 'Regex',
  Operator = 'Operator',
  SugarOperator = 'SugarOperator',
  PropertyAccess = 'PropertyAccess',
  BracketOpen = 'BracketOpen',
  BracketClose = 'BracketClose',
  BraceOpen = 'BraceOpen',
  BraceClose = 'BraceClose',
  ParenOpen = 'ParenOpen',
  ParenClose = 'ParenClose',
  Comma = 'Comma',
  Colon = 'Colon',

  Text = 'Text',
  Whitespace = 'Whitespace',
  Newline = 'Newline',
  LineComment = 'LineComment',
  BlockComment = 'BlockComment',
  HtmlComment = 'HtmlComment',
  Error = 'Error',
  EOF = 'EOF',
}

export interface Token {
  type: TokenType;
  value: string;
  range: SourceRange;
}

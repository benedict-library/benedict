-- Example deployment: a central library and one topic library whose wiki may only hold public knowledge.
INSERT INTO library.registry (key, ord, name, central, room, search, wiki_root, dirs, save_block) VALUES
  ('main', 1, 'Main library', true,  'main-search', NULL,   '/srv/library/main-wiki', NULL, NULL),
  ('shop', 2, 'Shop library', false, 'shop',        'room', '/srv/library/shop-wiki', 'entities,concepts,queries',
   '{"email": "[\\w.+-]+@[\\w-]+\\.[\\w.-]+", "work branch": "\\b(?:feat|fix|chore|docs|refactor)/[\\w-]+"}')
ON CONFLICT (key) DO NOTHING;

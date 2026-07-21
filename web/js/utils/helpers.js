/**
 * Pure helper functions, no Vue dependency.
 */

export function fmtTokens(n) {
  if (n >= 1000) return (n / 1000).toFixed(2).replace(/\.?0+$/, '') + 'K';
  return String(n);
}

export function renderMarkdown(text) {
  if (typeof marked === 'undefined') return text;
  return marked.parse(text, { breaks: true });
}

export function generateColors(n) {
  // iOS 系统色调色板
  const palette = [
    '#007AFF','#34C759','#FF9500','#FF3B30','#AF52DE',
    '#5AC8FA','#FF2D55','#5856D6','#FFCC00','#00C7BE'
  ];
  return Array.from({length: n}, (_, i) => palette[i % palette.length]);
}

export function hasPersonalInfo(pi) {
  if (!pi) return false;
  return !!(pi.age_range || pi.gender || pi.height_weight || pi.relationship
    || pi.hometown || pi.current_city || pi.education || pi.study_abroad
    || pi.profession || pi.income_hint || pi.car || pi.housing
    || pi.personality_traits || pi.political_stance || pi.confidence_note
    || (pi.sports_teams && pi.sports_teams.length)
    || (pi.hobbies && pi.hobbies.length)
    || (pi.games && pi.games.length)
    || (pi.other_clues && pi.other_clues.length));
}

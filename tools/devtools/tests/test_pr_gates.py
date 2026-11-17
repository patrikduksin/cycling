import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
GATES = ROOT / 'tools/pr-gates.sh'

NO_COMMENTS = {'type': 'tool_use', 'name': 'Agent', 'input': {'subagent_type': 'pstack:comment-sicko'}}
DESLOP_CALL = {'type': 'tool_use', 'name': 'Skill', 'input': {'skill': 'pstack:deslop'}}


def assistant(*blocks):
    return {'type': 'assistant', 'message': {'role': 'assistant', 'content': list(blocks)}}


def user(text):
    return {'type': 'user', 'message': {'role': 'user', 'content': text}}


def skill_result(text, **extra):
    block = {'type': 'tool_result', 'tool_use_id': 't1', 'content': text, **extra}
    return {'type': 'user', 'message': {'role': 'user', 'content': [block]}}


DESLOP = skill_result('Launching skill: pstack:deslop')


class PrGatesTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.config = Path(temporary.name)
        project = self.config / 'projects' / '-home-me-cycling'
        self.root = project / 's1.jsonl'
        self.subagents = project / 's1' / 'subagents'
        self.subagents.mkdir(parents=True)
        self.write(self.root, user('open the PR'))

    def write(self, path, *records):
        path.write_text(''.join(json.dumps(r, separators=(',', ':')) + '\n' for r in records))

    def gates(self, *gh_arguments, session='s1'):
        env = {'PATH': os.environ['PATH'], 'HOME': str(self.config), 'CLAUDE_CONFIG_DIR': str(self.config)}
        if session:
            env['CLAUDE_CODE_SESSION_ID'] = session
        return subprocess.run(['bash', GATES, *gh_arguments], env=env, capture_output=True, text=True)

    def refusal(self, *gh_arguments):
        result = self.gates(*(gh_arguments or ('pr', 'create', '--fill')))
        self.assertEqual(result.returncode, 1, result.stderr)
        return result.stderr

    def assert_allowed(self, *gh_arguments, **kwargs):
        result = self.gates(*gh_arguments, **kwargs)
        self.assertEqual((result.returncode, result.stdout), (0, ''), result.stderr)
        return result.stderr

    def test_other_gh_commands_pass_silently(self):
        for arguments in (
            ('pr', 'view', '7'),
            ('pr', 'comment', '7', '--body', 'run gh pr create next'),
            ('issue', 'create', '--title', 'pr create fails'),
            ('pr', 'list', '--search', 'create'),
            ('api', 'repos/o/r/pulls'),
            ('pr', 'create', '--dry-run', '--title', 'Preview'),
            ('pr', 'create', '--help'),
        ):
            with self.subTest(arguments):
                self.assertEqual(self.assert_allowed(*arguments), '')

    def test_gh_pr_merge_is_refused_in_any_session_and_names_cycling_merge(self):
        self.write(self.root, DESLOP, assistant(NO_COMMENTS))
        for session in ('s1', None):
            for arguments in (
                ('pr', 'merge', '7', '--squash'),
                ('-R', 'o/r', 'pr', 'merge'),
                ('pr', 'merge', '7', '--squash', '--body', '-h'),
            ):
                with self.subTest(arguments, session=session):
                    result = self.gates(*arguments, session=session)
                    self.assertEqual(result.returncode, 1)
                    self.assertIn('cycling merge <pr>', result.stderr)

    def test_no_evidence_refuses_and_names_both_gates(self):
        reason = self.refusal()
        self.assertIn('/deslop and /no-comments have not run', reason)
        self.assertIn('.agents/skills/verify/SKILL.md', reason)

    def test_every_spelling_of_pr_create_is_refused(self):
        for arguments in (
            ('pr', 'create'),
            ('pr', 'new', '--fill'),
            ('-R', 'o/r', 'pr', 'create'),
            ('--repo', 'o/r', 'pr', 'create'),
            ('--repo=o/r', 'pr', 'create'),
            ('-Ro/r', 'pr', 'create', '--title', 'a # b'),
            ('pr', 'create', '--title', 'Add b', '--body', '-h'),
            ('pr', 'create', '-t', '--help', '--fill'),
            ('pr', 'create', '--fill', '--base', '--dry-run'),
        ):
            with self.subTest(arguments):
                self.refusal(*arguments)

    def test_deslop_alone_names_only_the_comment_check(self):
        self.write(self.root, DESLOP)
        reason = self.refusal()
        self.assertIn('/no-comments has not run', reason)
        self.assertNotIn('/deslop', reason)

    def test_evidence_in_a_subagent_counts(self):
        self.write(self.root, DESLOP)
        self.write(self.subagents / 'agent-a1.jsonl', assistant(NO_COMMENTS))
        self.assertEqual(self.assert_allowed('pr', 'create'), '')

    def test_evidence_quoted_in_a_message_does_not_count(self):
        quoted = '"content":"Launching skill: pstack:deslop" and "subagent_type":"pstack:comment-sicko"'
        self.write(self.root, user(quoted), assistant({'type': 'text', 'text': quoted}))
        self.assertIn('/deslop and /no-comments have', self.refusal())

    def test_a_skill_call_that_failed_does_not_count(self):
        self.write(self.root, assistant(DESLOP_CALL), skill_result('Unknown skill: pstack:deslop', is_error=True))
        self.assertIn('/deslop and /no-comments have', self.refusal())

    def test_a_typed_slash_command_counts(self):
        typed = (
            '<command-message>pstack:no-comments</command-message>\n<command-name>/pstack:no-comments</command-name>'
        )
        self.write(self.root, DESLOP, user(typed))
        self.assertEqual(self.assert_allowed('pr', 'create'), '')

    def test_a_session_that_is_not_claude_is_allowed(self):
        self.assertEqual(self.assert_allowed('pr', 'create', session=None), '')

    def test_a_missing_transcript_allows_with_a_warning(self):
        self.assertIn('could not check', self.assert_allowed('pr', 'create', session='unknown'))


if __name__ == '__main__':
    unittest.main()

import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from cycling_devtools import playbook_audit


def quietly(function, *args, **kwargs):
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
        result = function(*args, **kwargs)
    return result, out.getvalue()


def record(kind, content, **extra):
    return json.dumps({'type': kind, 'message': {'role': kind, 'content': content}, **extra})


class DigestTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.config = Path(temporary.name)
        self.projects = self.config / 'projects'
        project = self.projects / '-home-me-cycling'
        (project / 's1' / 'subagents').mkdir(parents=True)
        (project / 's1.jsonl').write_text(
            '\n'.join(
                [
                    json.dumps({'type': 'queue-operation'}),
                    record('user', 'build the playbook audit'),
                    record('user', 'injected skill body', isMeta=True),
                    record(
                        'assistant',
                        [
                            {'type': 'thinking', 'thinking': 'secret musing'},
                            {'type': 'tool_use', 'name': 'Skill', 'input': {'skill': 'pstack:deslop', 'args': ''}},
                        ],
                    ),
                    record(
                        'assistant',
                        [
                            {
                                'type': 'tool_use',
                                'name': 'Agent',
                                'input': {'subagent_type': 'pstack:poteto-agent', 'model': 'opus', 'prompt': 'Read it'},
                            }
                        ],
                    ),
                    record('assistant', [{'type': 'tool_use', 'name': 'Bash', 'input': {'command': 'cycling test'}}]),
                    record('user', [{'type': 'tool_result', 'content': [{'type': 'text', 'text': 'all passed'}]}]),
                ]
            )
            + '\n'
        )
        (project / 's1' / 'subagents' / 'agent-x.jsonl').write_text(record('user', 'subagent brief') + '\n')

    def digest(self):
        code, out = quietly(playbook_audit.print_digest, 's1', self.projects)
        self.assertEqual(code, 0)
        return out

    def test_prints_each_action_with_its_line_number(self):
        out = self.digest()
        self.assertIn(str(self.projects / '-home-me-cycling' / 's1.jsonl'), out.splitlines()[0])
        for line in (
            'L2 user: build the playbook audit',
            'L4 call Skill: skill=pstack:deslop',
            'L5 call Agent: subagent_type=pstack:poteto-agent model=opus prompt=Read it',
            'L6 call Bash: command=cycling test',
            'L7 result: all passed',
        ):
            self.assertIn(line, out)
        self.assertNotIn('secret musing', out)
        self.assertNotIn('injected skill body', out)

    def test_subagents_follow_the_root_under_their_own_header(self):
        out = self.digest()
        header = out.index('agent-x.jsonl')
        self.assertGreater(header, out.index('L7 result'))
        self.assertIn('L1 user: subagent brief', out[header:])

    def test_an_unknown_session_fails(self):
        code, _ = quietly(playbook_audit.print_digest, 'nope', self.projects)
        self.assertEqual(code, 1)

    def test_the_task_prints_the_digest_of_the_named_session(self):
        with patch.dict(os.environ, {'CLAUDE_CONFIG_DIR': str(self.config)}):
            code, out = quietly(playbook_audit.main, ['--session', 's1'])
        self.assertEqual(code, 0)
        self.assertIn('L2 user: build the playbook audit', out)


if __name__ == '__main__':
    unittest.main()

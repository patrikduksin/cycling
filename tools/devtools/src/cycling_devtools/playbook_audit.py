"""Print a session's transcript and its subagents', one line per block, for docs/agents/playbook-auditor.md."""

import argparse
import json
import os
import sys
from pathlib import Path


def head(text, limit):
    text = ' '.join(text.split())
    return text if len(text) <= limit else text[:limit] + '…'


def fields(tool_input, *names, limit=160):
    return ' '.join(f'{name}={head(str(tool_input[name]), limit)}' for name in names if name in tool_input)


def whole_todo_items(tool_input):
    return ' | '.join(f'[{t.get("status")}] {t.get("content")}' for t in tool_input.get('todos', []))


CALLS = {
    'Skill': lambda i: fields(i, 'skill', 'args'),
    'Agent': lambda i: fields(i, 'subagent_type', 'model', 'effort', 'description', 'prompt'),
    'Bash': lambda i: fields(i, 'command', limit=240),
    'Edit': lambda i: fields(i, 'file_path'),
    'Write': lambda i: fields(i, 'file_path'),
    'Read': lambda i: fields(i, 'file_path'),
    'TodoWrite': whole_todo_items,
    'TaskCreate': lambda i: fields(i, 'subject', limit=400),
}


def describe_call(name, tool_input):
    summary = CALLS.get(name)
    return summary(tool_input) if summary else head(json.dumps(tool_input), 300)


def result_text(content):
    if isinstance(content, str):
        return content
    return ' '.join(block.get('text', '') for block in content or [] if block.get('type') == 'text')


def record_lines(record):
    if record.get('type') not in {'user', 'assistant'} or record.get('isMeta'):
        return
    content = record.get('message', {}).get('content')
    if isinstance(content, str):
        content = [{'type': 'text', 'text': content}]
    for block in content or []:
        kind = block.get('type')
        if kind == 'text':
            limit = 400 if record['type'] == 'user' else 600
            yield f'{record["type"]}: {head(block["text"], limit)}'
        elif kind == 'tool_use':
            yield f'call {block["name"]}: {describe_call(block["name"], block.get("input", {}))}'
        elif kind == 'tool_result':
            label = 'error' if block.get('is_error') else 'result'
            yield f'{label}: {head(result_text(block.get("content")), 200)}'


def digest(path):
    with path.open(errors='replace') as transcript:
        for number, line in enumerate(transcript, 1):
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            for text in record_lines(record):
                yield f'L{number} {text}'


def resolved_once(paths):
    return sorted({path.resolve() for path in paths})


def transcripts(session, projects):
    roots = resolved_once(projects.glob(f'*/{session}.jsonl'))
    if not roots:
        return []
    return [roots[0], *sorted(roots[0].with_suffix('').glob('subagents/agent-*.jsonl'))]


def subagent_header(path):
    meta = path.with_suffix('.meta.json')
    try:
        about = json.loads(meta.read_text())
    except OSError, json.JSONDecodeError:
        return f'== subagent {path}'
    return f'== subagent {path} ({about.get("agentType")}: {about.get("description")})'


def print_digest(session, projects):
    paths = transcripts(session, projects)
    if not paths:
        print(f'no transcript for session {session} under {projects}', file=sys.stderr)
        return 1
    print(f'== session {paths[0]}')
    print('\n'.join(digest(paths[0])))
    for path in paths[1:]:
        print(f'\n{subagent_header(path)}')
        print('\n'.join(digest(path)))
    return 0


def claude_projects():
    return Path(os.environ.get('CLAUDE_CONFIG_DIR', Path.home() / '.claude')) / 'projects'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--session', default=os.environ.get('CLAUDE_CODE_SESSION_ID'))
    args = parser.parse_args(argv)
    if not args.session:
        parser.error('--session is required when CLAUDE_CODE_SESSION_ID is unset')
    return print_digest(args.session, claude_projects())


if __name__ == '__main__':
    sys.exit(main())

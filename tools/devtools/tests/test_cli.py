"""The cycling task runner: every task resolves, and arguments reach the right step."""

import importlib.util
import re
import subprocess
import unittest

from cycling_devtools import cli


class TaskTableTests(unittest.TestCase):
    def test_names_and_aliases_are_unique(self):
        names = [t.name for t in cli.TASKS] + [t.alias for t in cli.TASKS if t.alias]
        self.assertEqual(len(names), len(set(names)))

    def test_needs_name_tasks(self):
        for task in cli.TASKS:
            for need in task.needs:
                self.assertIn(need, cli.BY_NAME, task.name)

    def test_every_devtools_module_exists(self):
        for task in cli.TASKS:
            for module in re.findall(r'-m (cycling_devtools\.[\w.]+)', ' '.join(task.steps)):
                self.assertIsNotNone(importlib.util.find_spec(module), f'{task.name}: {module}')


class ArgumentTests(unittest.TestCase):
    def echo(self, *steps, args=()):
        task = cli.Task('t', 'test', steps)
        argvs = [cli.bash(step, task, list(args), last=i == len(steps) - 1) for i, step in enumerate(steps)]
        return [subprocess.run(argv, capture_output=True, text=True, check=True).stdout for argv in argvs]

    def test_arguments_go_to_the_last_step_only(self):
        self.assertEqual(self.echo('echo first', 'echo last', args=['a b', 'c']), ['first\n', 'last a b c\n'])

    def test_a_step_that_names_its_arguments_gets_no_extra_copy(self):
        self.assertEqual(self.echo('echo --pr "$1"', args=['7']), ['--pr 7\n'])

    def test_unknown_task_lists_the_tasks(self):
        self.assertEqual(cli.main(['no-such-task']), 2)


if __name__ == '__main__':
    unittest.main()

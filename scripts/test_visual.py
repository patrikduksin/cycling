import unittest

from visual import PIXELS, compare, compare_regions


class VisualComparisonTests(unittest.TestCase):
    def test_masks_are_explicit_and_unmasked_changes_fail(self):
        expected = [0x0863] * PIXELS
        actual = expected.copy()
        actual[10 * 80 + 20] = 0xffff
        compare(actual, expected, masks=((20, 10, 1, 1),))
        with self.assertRaises(AssertionError):
            compare(actual, expected)

    def test_rgb_channel_tolerance_is_bounded(self):
        expected = [0] * PIXELS
        actual = expected.copy()
        actual[0] = (1 << 11) | (2 << 5) | 1
        compare(actual, expected, tolerance=(1, 2, 1))
        with self.assertRaises(AssertionError):
            compare(actual, expected, tolerance=(0, 2, 1))

    def test_static_regions_remain_exact_even_inside_a_dynamic_mask(self):
        expected = [0] * PIXELS
        actual = expected.copy()
        actual[10 * 80 + 20] = 0xffff
        compare(actual, expected, masks=((20, 10, 1, 1),))
        with self.assertRaises(AssertionError):
            compare_regions(actual, expected, ((20, 10, 1, 1),))

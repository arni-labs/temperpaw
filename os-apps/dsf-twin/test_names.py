"""Exercise the tenant-wide short-name boundary with real CSDL parsing."""

import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

from check_names import Names, check, names

EDM = "{http://docs.oasis-open.org/odata/ns/edm}"
CSDL = Path(__file__).resolve().parent / "specs" / "model.csdl.xml"


def model(*schemas: str) -> bytes:
    return (
        '<Edmx xmlns="http://docs.oasis-open.org/odata/ns/edm">'
        + "".join(schemas)
        + "</Edmx>"
    ).encode()


class NamesTest(unittest.TestCase):
    def test_unrelated_live_duplicates_do_not_block_unique_new_names(self) -> None:
        live = names(
            model(
                '<EntityType Name="Agent"/><EntityType Name="Agent"/><EntitySet Name="Agents"/>'
            ),
            candidate=False,
        )
        check(Names(frozenset({"DsfThing"}), frozenset({"DsfThings"})), live)

    def test_ambiguous_live_name_cannot_be_claimed_by_previous_bundle(self) -> None:
        live = names(
            model(
                '<EntityType Name="DsfThing"/><EntityType Name="DsfThing"/><EntitySet Name="DsfThings"/>'
            ),
            candidate=False,
        )
        owned = Names(frozenset({"DsfThing"}), frozenset({"DsfThings"}))
        with self.assertRaisesRegex(ValueError, "ambiguous ownership"):
            check(owned, live, owned)

    def test_namespace_does_not_hide_type_collision(self) -> None:
        xml = model(
            '<Schema Namespace="A"><EntityType Name="DsfThing"/><EntitySet Name="DsfThings"/></Schema>',
            '<Schema Namespace="B"><EntityType Name="DsfThing"/></Schema>',
        )
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            names(xml, candidate=True)

    def test_entity_sets_collide_across_containers(self) -> None:
        xml = model(
            '<EntityType Name="DsfThing"/><EntityContainer Name="A"><EntitySet Name="DsfThings"/></EntityContainer><EntityContainer Name="B"><EntitySet Name="DsfThings"/></EntityContainer>'
        )
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            names(xml, candidate=True)

    def test_first_install_preserves_existing_dsf_deploy(self) -> None:
        check(
            Names(
                frozenset({"DsfRailwayServiceInstance"}),
                frozenset({"DsfRailwayServiceInstances"}),
            ),
            Names(frozenset({"DsfDeploy"}), frozenset({"DsfDeploys"})),
        )

    def test_existing_prefix_does_not_prove_ownership(self) -> None:
        same = Names(frozenset({"DsfDeploy"}), frozenset({"DsfDeploys"}))
        with self.assertRaisesRegex(ValueError, "owned outside"):
            check(same, same)

    def test_upgrade_can_only_reuse_previous_owned_names(self) -> None:
        own = Names(frozenset({"DsfThing"}), frozenset({"DsfThings"}))
        live = Names(own.types | {"DsfDeploy"}, own.sets | {"DsfDeploys"})
        check(own, live, own)
        with self.assertRaisesRegex(ValueError, "owned outside"):
            check(live, live, own)

    def test_app_cannot_redeclare_dependency(self) -> None:
        with self.assertRaisesRegex(ValueError, "unprefixed"):
            names(
                model('<EntityType Name="Effort"/><EntitySet Name="Efforts"/>'),
                candidate=True,
            )


class TwinAnnotationsTest(unittest.TestCase):
    """The generated schema declares the twin and its graph edges."""

    def setUp(self) -> None:
        self.schema = ET.parse(CSDL).find(f".//{EDM}Schema")

    def annotations(self, element) -> dict:
        return {
            a.get("Term"): a.get("String")
            for a in element.findall(f"{EDM}Annotation")
        }

    def entity(self, name: str):
        for et in self.schema.findall(f"{EDM}EntityType"):
            if et.get("Name") == name:
                return et
        raise AssertionError(f"missing entity type {name}")

    def target(self, path: str):
        for block in self.schema.findall(f"{EDM}Annotations"):
            if block.get("Target") == path:
                return block
        raise AssertionError(f"missing annotation target {path}")

    def property_names(self, entity_name: str) -> set:
        return {p.get("Name") for p in self.entity(entity_name).findall(f"{EDM}Property")}

    def test_schema_carries_twin_marker(self) -> None:
        self.assertEqual(self.annotations(self.schema).get("Temper.Twin"), "Deep Sci-Fi")

    def test_dependency_ids_carries_references(self) -> None:
        # The property itself stays a self-closing element the kernel parser reads.
        self.assertIn("DependencyIds", self.property_names("DsfRailwayServiceInstance"))
        annotations = self.annotations(
            self.target("Dsf.Twin.DsfRailwayServiceInstance/DependencyIds")
        )
        self.assertIn("DsfMediaPipeline", annotations["Temper.References"])
        self.assertEqual(annotations["Temper.ReferenceShape"], "json_list")

    def test_resource_type_declares_its_provider(self) -> None:
        annotations = self.annotations(self.entity("DsfCloudflareR2Bucket"))
        self.assertEqual(annotations["Temper.Provider"], "cloudflare")
        self.assertEqual(annotations["Temper.Role"], "resource")

    def test_a_monitor_watches_resources_and_is_not_one(self) -> None:
        # A Datadog monitor is provider-managed like a resource, but it
        # observes the app rather than serving it; the graph draws it apart.
        annotations = self.annotations(self.entity("DsfDatadogMonitor"))
        self.assertEqual(annotations["Temper.Provider"], "datadog")
        self.assertEqual(annotations["Temper.Role"], "monitor")


if __name__ == "__main__":
    unittest.main()

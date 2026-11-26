from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.ops_orgs_orgs_item import OpsOrgsOrgsItem
    from ..models.governance import Governance


T = TypeVar("T", bound="OpsOrgs")


@_attrs_define
class OpsOrgs:
    """`GET /ops/orgs`: a page of organizations and the releases approved to hold their apps.

    Attributes:
        fresh (bool):
        governance (Governance): Who decides what on this cluster: release approvals, app upgrades and outbound
            approvals (`owner`: each app's owner; `operator`: the operator), and, left out at their defaults, `deployments`
            (`open`), `deployers` (`operator`) and `signUp` (`open`).
        orgs (list['OpsOrgsOrgsItem']):
        next_ (Union[None, str]):
    """

    fresh: bool
    governance: "Governance"
    orgs: list["OpsOrgsOrgsItem"]
    next_: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.ops_orgs_orgs_item import OpsOrgsOrgsItem
        from ..models.governance import Governance

        fresh = self.fresh

        governance = self.governance.to_dict()

        orgs = []
        for orgs_item_data in self.orgs:
            orgs_item = orgs_item_data.to_dict()
            orgs.append(orgs_item)

        next_: Union[None, str]
        next_ = self.next_

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "fresh": fresh,
                "governance": governance,
                "orgs": orgs,
                "next": next_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.ops_orgs_orgs_item import OpsOrgsOrgsItem
        from ..models.governance import Governance

        d = dict(src_dict)
        fresh = d.pop("fresh")

        governance = Governance.from_dict(d.pop("governance"))

        orgs = []
        _orgs = d.pop("orgs")
        for orgs_item_data in _orgs:
            orgs_item = OpsOrgsOrgsItem.from_dict(orgs_item_data)

            orgs.append(orgs_item)

        def _parse_next_(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        next_ = _parse_next_(d.pop("next"))

        ops_orgs = cls(
            fresh=fresh,
            governance=governance,
            orgs=orgs,
            next_=next_,
        )

        ops_orgs.additional_properties = d
        return ops_orgs

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties

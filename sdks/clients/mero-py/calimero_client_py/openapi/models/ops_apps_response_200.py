from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.governance import Governance
    from ..models.report_app import ReportApp


T = TypeVar("T", bound="OpsAppsResponse200")


@_attrs_define
class OpsAppsResponse200:
    """
    Attributes:
        fresh (bool):
        governance (Governance): Who decides what on this cluster: release approvals, app upgrades and outbound
            approvals (`owner`: each app's owner; `operator`: the operator), and, left out at their defaults, `deployments`
            (`open`), `deployers` (`operator`) and `signUp` (`open`).
        apps (list['ReportApp']):
        next_ (Union[None, str]):
    """

    fresh: bool
    governance: "Governance"
    apps: list["ReportApp"]
    next_: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.governance import Governance
        from ..models.report_app import ReportApp

        fresh = self.fresh

        governance = self.governance.to_dict()

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        next_: Union[None, str]
        next_ = self.next_

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "fresh": fresh,
                "governance": governance,
                "apps": apps,
                "next": next_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.governance import Governance
        from ..models.report_app import ReportApp

        d = dict(src_dict)
        fresh = d.pop("fresh")

        governance = Governance.from_dict(d.pop("governance"))

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = ReportApp.from_dict(apps_item_data)

            apps.append(apps_item)

        def _parse_next_(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        next_ = _parse_next_(d.pop("next"))

        ops_apps_response_200 = cls(
            fresh=fresh,
            governance=governance,
            apps=apps,
            next_=next_,
        )

        ops_apps_response_200.additional_properties = d
        return ops_apps_response_200

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

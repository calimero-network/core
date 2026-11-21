from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.rollout import Rollout


T = TypeVar("T", bound="RolloutResumeResponse200")


@_attrs_define
class RolloutResumeResponse200:
    """
    Attributes:
        deployment (str):
        apps (list['Rollout']):
    """

    deployment: str
    apps: list["Rollout"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.rollout import Rollout

        deployment = self.deployment

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deployment": deployment,
                "apps": apps,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.rollout import Rollout

        d = dict(src_dict)
        deployment = d.pop("deployment")

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = Rollout.from_dict(apps_item_data)

            apps.append(apps_item)

        rollout_resume_response_200 = cls(
            deployment=deployment,
            apps=apps,
        )

        rollout_resume_response_200.additional_properties = d
        return rollout_resume_response_200

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

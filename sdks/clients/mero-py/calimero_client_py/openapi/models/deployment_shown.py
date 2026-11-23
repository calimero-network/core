from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.deployment_shown_owner import DeploymentShownOwner


T = TypeVar("T", bound="DeploymentShown")


@_attrs_define
class DeploymentShown:
    """The deployment an instance comes from, as its customers are shown it: names are not unique, so always with its id
    and its owner's kind and name (`null` once the owner is gone).

        Attributes:
            id (str):
            name (str):
            owner (DeploymentShownOwner):
    """

    id: str
    name: str
    owner: "DeploymentShownOwner"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployment_shown_owner import DeploymentShownOwner

        id = self.id

        name = self.name

        owner = self.owner.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "name": name,
                "owner": owner,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployment_shown_owner import DeploymentShownOwner

        d = dict(src_dict)
        id = d.pop("id")

        name = d.pop("name")

        owner = DeploymentShownOwner.from_dict(d.pop("owner"))

        deployment_shown = cls(
            id=id,
            name=name,
            owner=owner,
        )

        deployment_shown.additional_properties = d
        return deployment_shown

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

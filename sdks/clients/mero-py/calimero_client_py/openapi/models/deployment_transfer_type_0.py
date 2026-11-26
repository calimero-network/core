from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.deployer_type_0 import DeployerType0
    from ..models.deployer_type_1 import DeployerType1


T = TypeVar("T", bound="DeploymentTransferType0")


@_attrs_define
class DeploymentTransferType0:
    """
    Attributes:
        to (Union['DeployerType0', 'DeployerType1']): An account or an organization.
        offered_by (str):
        offered_at (str):
        expires_at (str):
    """

    to: Union["DeployerType0", "DeployerType1"]
    offered_by: str
    offered_at: str
    expires_at: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_type_1 import DeployerType1

        to: dict[str, Any]
        if isinstance(self.to, DeployerType0):
            to = self.to.to_dict()
        else:
            to = self.to.to_dict()

        offered_by = self.offered_by

        offered_at = self.offered_at

        expires_at = self.expires_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "to": to,
                "offeredBy": offered_by,
                "offeredAt": offered_at,
                "expiresAt": expires_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_type_1 import DeployerType1

        d = dict(src_dict)

        def _parse_to(data: object) -> Union["DeployerType0", "DeployerType1"]:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_deployer_type_0 = DeployerType0.from_dict(data)

                return componentsschemas_deployer_type_0
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_deployer_type_1 = DeployerType1.from_dict(data)

            return componentsschemas_deployer_type_1

        to = _parse_to(d.pop("to"))

        offered_by = d.pop("offeredBy")

        offered_at = d.pop("offeredAt")

        expires_at = d.pop("expiresAt")

        deployment_transfer_type_0 = cls(
            to=to,
            offered_by=offered_by,
            offered_at=offered_at,
            expires_at=expires_at,
        )

        deployment_transfer_type_0.additional_properties = d
        return deployment_transfer_type_0

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

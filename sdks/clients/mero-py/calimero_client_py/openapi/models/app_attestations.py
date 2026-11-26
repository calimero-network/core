from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.replica_attestation_type_0 import ReplicaAttestationType0
    from ..models.replica_attestation_type_1 import ReplicaAttestationType1


T = TypeVar("T", bound="AppAttestations")


@_attrs_define
class AppAttestations:
    """`GET /apps/{app}/attestation`: the app, the cluster each replica's quote must bind, and each replica's receipt-key
    attestation (or why it could not be fetched).

        Attributes:
            app_id (str):
            cluster (str):
            replicas (list[Union['ReplicaAttestationType0', 'ReplicaAttestationType1']]):
    """

    app_id: str
    cluster: str
    replicas: list[Union["ReplicaAttestationType0", "ReplicaAttestationType1"]]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.replica_attestation_type_0 import ReplicaAttestationType0
        from ..models.replica_attestation_type_1 import ReplicaAttestationType1

        app_id = self.app_id

        cluster = self.cluster

        replicas = []
        for replicas_item_data in self.replicas:
            replicas_item: dict[str, Any]
            if isinstance(replicas_item_data, ReplicaAttestationType0):
                replicas_item = replicas_item_data.to_dict()
            else:
                replicas_item = replicas_item_data.to_dict()

            replicas.append(replicas_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "appId": app_id,
                "cluster": cluster,
                "replicas": replicas,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.replica_attestation_type_0 import ReplicaAttestationType0
        from ..models.replica_attestation_type_1 import ReplicaAttestationType1

        d = dict(src_dict)
        app_id = d.pop("appId")

        cluster = d.pop("cluster")

        replicas = []
        _replicas = d.pop("replicas")
        for replicas_item_data in _replicas:

            def _parse_replicas_item(
                data: object,
            ) -> Union["ReplicaAttestationType0", "ReplicaAttestationType1"]:
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_replica_attestation_type_0 = (
                        ReplicaAttestationType0.from_dict(data)
                    )

                    return componentsschemas_replica_attestation_type_0
                except:  # noqa: E722
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_replica_attestation_type_1 = (
                    ReplicaAttestationType1.from_dict(data)
                )

                return componentsschemas_replica_attestation_type_1

            replicas_item = _parse_replicas_item(replicas_item_data)

            replicas.append(replicas_item)

        app_attestations = cls(
            app_id=app_id,
            cluster=cluster,
            replicas=replicas,
        )

        app_attestations.additional_properties = d
        return app_attestations

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
